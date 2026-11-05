//! Tilt steering: turn the phone like a steering wheel.
//!
//! The sensor gives the direction of "up" in the device's own frame (x to
//! the right and y to the top of the screen in its natural portrait
//! orientation, z out of the screen). Steering is the rotation about the
//! screen's normal: the angle of "up" within the screen plane, measured from
//! where the player held the phone when driving started (or after
//! [`TiltSteering::recentre`]). How far the phone leans back does not
//! matter, and as it nears flat (where that angle means nothing) steering
//! fades to centre.
//!
//! Which edge of the device is the screen's top is worked out from the same
//! reading: the device axis nearest to "up", kept until the phone turns well
//! past it, so either landscape orientation (or portrait) works without
//! asking the OS.

use glam::{Vec2, Vec3};

/// How tilt maps to steering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TiltSettings {
    /// Turn the phone this far for full steering, degrees.
    pub full_deg: f32,
    /// Turns smaller than this are ignored (hand shake), degrees.
    pub dead_deg: f32,
    /// Smoothing of the sensor reading, s.
    pub smoothing_s: f32,
    /// Response curve: steering = share of the turn ^ this. Above 1 makes
    /// small turns gentler (steady on a motorway) while a full turn still
    /// gives full steering.
    pub curve: f32,
}

impl Default for TiltSettings {
    fn default() -> Self {
        TiltSettings {
            full_deg: 25.0,
            dead_deg: 1.5,
            smoothing_s: 0.05,
            curve: 1.5,
        }
    }
}

/// Below this share of "up" lying in the screen plane, the phone is too
/// flat to steer with; above the second, it steers fully.
const FLAT: f32 = 0.2;
const STEEP: f32 = 0.4;
/// The screen's top edge is re-chosen once the phone turns this far past it.
const SWITCH_DEG: f32 = 70.0;

/// The device axes that can be the screen's top: +y portrait, +x and −x
/// landscape, −y upside down.
const AXES: [Vec2; 4] = [Vec2::Y, Vec2::X, Vec2::NEG_X, Vec2::NEG_Y];

/// Tilt steering's state between readings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TiltSteering {
    pub settings: TiltSettings,
    /// Smoothed "up", device frame (unit length).
    up: Option<Vec3>,
    /// Which of [`AXES`] is the screen's top.
    axis: Option<usize>,
    /// Angle that counts as straight ahead, rad; `None` takes the next reading.
    neutral: Option<f32>,
}

impl TiltSteering {
    pub fn new(settings: TiltSettings) -> Self {
        TiltSteering {
            settings,
            ..Default::default()
        }
    }

    /// Make the way the phone is held now straight ahead.
    pub fn recentre(&mut self) {
        self.neutral = None;
    }

    /// The screen's top edge as a device axis, once known.
    pub fn screen_up(&self) -> Option<Vec2> {
        self.axis.map(|a| AXES[a])
    }

    /// One reading: `up` in the device frame (any length; the accelerometer
    /// at rest or the gravity sensor's reaction), `dt` s after the last.
    /// Returns steering, −1 (right) to 1 (left).
    pub fn update(&mut self, up: Vec3, dt: f32) -> f32 {
        let Some(reading) = up.try_normalize() else {
            return 0.0;
        };
        let k = 1.0 - (-dt.max(0.0) / self.settings.smoothing_s.max(1e-4)).exp();
        let u = match self.up {
            Some(prev) => prev.lerp(reading, k).try_normalize().unwrap_or(reading),
            None => reading,
        };
        self.up = Some(u);
        let plane = u.truncate();
        let share = plane.length();
        if share < FLAT {
            return 0.0;
        }
        // The screen's top: the nearest axis, kept until well past it.
        let angle_from = |a: usize| angle_in_screen(plane, AXES[a]);
        let nearest = (0..AXES.len())
            .min_by(|&a, &b| angle_from(a).abs().total_cmp(&angle_from(b).abs()))
            .unwrap_or(0);
        match self.axis {
            Some(a) if angle_from(a).abs() < SWITCH_DEG.to_radians() => {}
            Some(_) if share < STEEP => {}
            _ => {
                if self.axis.is_some() {
                    self.neutral = None;
                }
                self.axis = Some(nearest);
            }
        }
        let theta = angle_from(self.axis.unwrap_or(nearest));
        let neutral = *self.neutral.get_or_insert(theta);
        let turn = wrap(theta - neutral).to_degrees();
        let s = &self.settings;
        let past = (turn.abs() - s.dead_deg).max(0.0) / (s.full_deg - s.dead_deg).max(1e-3);
        let weight = smoothstep(FLAT, STEEP, share);
        turn.signum() * past.min(1.0).powf(s.curve.max(0.1)) * weight
    }
}

/// Angle of `plane` (the in-screen part of "up") from the screen top `top`,
/// rad, positive when the phone is turned anticlockwise (left), as seen by
/// the player.
fn angle_in_screen(plane: Vec2, top: Vec2) -> f32 {
    // The screen's right edge is the top turned a quarter clockwise.
    let right = Vec2::new(top.y, -top.x);
    plane.dot(right).atan2(plane.dot(top))
}

fn wrap(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// "Up" in the device frame for a phone held in landscape (its top edge to
/// the player's left), leaning back `lean_deg` from upright and turned
/// `turn_deg` anticlockwise like a wheel. For fake sensors and tests.
pub fn held_landscape(turn_deg: f32, lean_deg: f32) -> Vec3 {
    let (turn, lean) = (turn_deg.to_radians(), lean_deg.to_radians());
    // Screen frame (right, up, out): upright is +up; leaning back tips it
    // towards +out; turning anticlockwise swings it towards +right.
    let screen = Vec3::new(turn.sin() * lean.cos(), turn.cos() * lean.cos(), lean.sin());
    // Landscape with the top edge left: screen up is device +x, screen
    // right is device −y.
    Vec3::new(screen.y, -screen.x, screen.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(t: &mut TiltSteering, up: Vec3) -> f32 {
        let mut s = 0.0;
        for _ in 0..60 {
            s = t.update(up, 1.0 / 60.0);
        }
        s
    }

    #[test]
    fn turning_like_a_wheel_steers() {
        let mut t = TiltSteering::default();
        assert_eq!(settle(&mut t, held_landscape(0.0, 40.0)), 0.0);
        assert_eq!(t.screen_up(), Some(Vec2::X));
        // Half the turn gives about a third of the steering (the curve).
        let half = settle(&mut t, held_landscape(13.25, 40.0));
        assert!((half - 0.5f32.powf(1.5)).abs() < 0.02, "{half}");
        assert_eq!(settle(&mut t, held_landscape(-40.0, 40.0)), -1.0);
        // Small wobbles do nothing.
        assert_eq!(settle(&mut t, held_landscape(1.0, 40.0)), 0.0);
    }

    #[test]
    fn how_far_it_leans_back_does_not_matter() {
        for lean in [0.0, 30.0, 60.0] {
            let mut t = TiltSteering::default();
            settle(&mut t, held_landscape(0.0, lean));
            let s = settle(&mut t, held_landscape(10.0, lean));
            assert!(
                (s - (8.5f32 / 23.5).powf(1.5)).abs() < 0.02,
                "lean {lean}: {s}"
            );
        }
    }

    #[test]
    fn straight_ahead_is_how_it_was_held() {
        let mut t = TiltSteering::default();
        // Held a little turned at the start: that is straight ahead.
        assert_eq!(settle(&mut t, held_landscape(8.0, 40.0)), 0.0);
        assert!(settle(&mut t, held_landscape(0.0, 40.0)) < -0.1);
        t.recentre();
        assert_eq!(settle(&mut t, held_landscape(0.0, 40.0)), 0.0);
    }

    #[test]
    fn either_landscape_steers_the_same_way() {
        // The other landscape: the device turned half round in the screen plane.
        let flipped = |turn: f32| {
            let u = held_landscape(turn, 40.0);
            Vec3::new(-u.x, -u.y, u.z)
        };
        let mut t = TiltSteering::default();
        settle(&mut t, flipped(0.0));
        assert_eq!(t.screen_up(), Some(Vec2::NEG_X));
        assert!(settle(&mut t, flipped(15.0)) > 0.3);
    }

    #[test]
    fn flat_on_a_table_steers_nothing() {
        let mut t = TiltSteering::default();
        settle(&mut t, held_landscape(0.0, 40.0));
        assert_eq!(settle(&mut t, held_landscape(20.0, 85.0)), 0.0);
        assert_eq!(settle(&mut t, Vec3::Z), 0.0);
    }

    #[test]
    fn readings_are_smoothed() {
        let mut t = TiltSteering::default();
        settle(&mut t, held_landscape(0.0, 40.0));
        let first = t.update(held_landscape(25.0, 40.0), 1.0 / 60.0);
        assert!(first > 0.02 && first < 0.4, "{first}");
    }
}
