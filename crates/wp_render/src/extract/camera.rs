//! Cameras (chase, top, side, showcase) and lighting/atmosphere settings.

#[allow(unused_imports)]
use super::*;

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    /// World direction that appears upwards on screen.
    pub up: Vec3,
    pub fov_y_deg: f32,
}

impl Camera {
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        // wgpu clip space matches DirectX: y up, depth 0..1. Reversed-Z with an
        // infinite far plane: depth 1 at the near plane, 0 at the horizon, which
        // keeps precision even for road markings centimetres above the asphalt.
        let view = glam::camera::rh::view::look_at_mat4(self.eye, self.target, self.up);
        let proj = glam::camera::rh::proj::directx::perspective_infinite_reverse(
            self.fov_y_deg.to_radians(),
            aspect,
            0.1,
        );
        proj * view
    }
}

/// Lighting and atmosphere. Defaults are a clear early afternoon.
#[derive(Clone, Copy, Debug)]
pub struct Environment {
    /// Direction towards the sun.
    pub sun_dir: Vec3,
    pub sun_intensity: f32,
    pub sun_color: Vec3,
    pub sky_zenith: Vec3,
    pub sky_horizon: Vec3,
    pub ground_base: Vec3,
    pub ground_line: Vec3,
    pub fog_density: f32,
    pub exposure: f32,
}

impl Default for Environment {
    fn default() -> Self {
        let elevation = 42f32.to_radians();
        let azimuth = 215f32.to_radians();
        Self {
            sun_dir: Vec3::new(
                azimuth.cos() * elevation.cos(),
                azimuth.sin() * elevation.cos(),
                elevation.sin(),
            ),
            sun_intensity: 2.9,
            sun_color: Vec3::new(1.0, 0.95, 0.88),
            sky_zenith: Vec3::new(0.16, 0.34, 0.78),
            sky_horizon: Vec3::new(0.62, 0.72, 0.86),
            ground_base: Vec3::new(0.055, 0.057, 0.062),
            ground_line: Vec3::new(0.5, 0.5, 0.46),
            fog_density: 0.0012,
            exposure: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraMode {
    /// Behind and above the player, looking ahead.
    #[default]
    Chase,
    /// Straight down, north up; higher for longer vehicles.
    Top,
    /// Level with the player, from its left.
    Side,
    /// Front three-quarter view, for screenshots of the vehicle.
    Showcase,
}

impl CameraMode {
    pub const ALL: [(&'static str, CameraMode); 4] = [
        ("chase", CameraMode::Chase),
        ("top", CameraMode::Top),
        ("side", CameraMode::Side),
        ("showcase", CameraMode::Showcase),
    ];

    pub fn parse(s: &str) -> Option<CameraMode> {
        Self::ALL.iter().find(|(n, _)| *n == s).map(|(_, m)| *m)
    }

    pub fn next(self) -> CameraMode {
        let i = Self::ALL.iter().position(|(_, m)| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()].1
    }

    /// Camera for a subject at `pose`, `length` metres long.
    pub fn camera(self, pose: &Pose, length: f32) -> Camera {
        let pos = pose.position.extend(0.0);
        let fwd = pose.forward().extend(0.0);
        let left = Vec3::Z.cross(fwd);
        let s = (length / 4.2).max(1.0);
        let (eye, target) = match self {
            CameraMode::Chase => (
                pos - fwd * 7.5 * s + Vec3::Z * 2.8 * s,
                pos + fwd * 2.5 * s + Vec3::Z * 0.9 * s,
            ),
            CameraMode::Top => (pos + Vec3::Z * (18.0 + 2.2 * length), pos),
            CameraMode::Side => (
                pos + left * 9.0 * s + Vec3::Z * 1.4 * s,
                pos + Vec3::Z * 0.6 * s,
            ),
            CameraMode::Showcase => (
                pos + fwd * 5.2 * s + left * 3.6 * s + Vec3::Z * 1.7 * s,
                pos + Vec3::Z * 0.55 * s - fwd * 0.3 * s,
            ),
        };
        let up = if self == CameraMode::Top {
            Vec3::Y
        } else {
            Vec3::Z
        };
        Camera {
            eye,
            target,
            up,
            fov_y_deg: 55.0,
        }
    }
}

/// Chase-camera smoothing for interactive windows. The camera's heading
/// trails the vehicle's, so steering corrections and the car's own yaw do not
/// swing the whole view each frame. Lives with the window, never in the
/// simulation; headless shots use the vehicle's exact heading.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChaseSmoothing {
    heading: Option<f32>,
}

impl ChaseSmoothing {
    /// Time constant of the trailing, seconds.
    const LAG_S: f32 = 0.25;

    /// Advance by `dt` seconds towards `target` heading (rad); the heading to draw with.
    pub fn update(&mut self, target: f32, dt: f32) -> f32 {
        let h = match self.heading {
            None => target,
            Some(h) => {
                let diff = (target - h + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                h + diff * (1.0 - (-dt / Self::LAG_S).exp())
            }
        };
        self.heading = Some(h);
        h
    }

    /// Forget the past (after a reset or teleport).
    pub fn snap(&mut self) {
        self.heading = None;
    }
}

#[cfg(test)]
mod smoothing_tests {
    use super::ChaseSmoothing;

    #[test]
    fn trails_then_settles_and_wraps_the_short_way() {
        let mut s = ChaseSmoothing::default();
        assert_eq!(s.update(0.0, 0.016), 0.0);
        let first = s.update(1.0, 0.016);
        assert!(first > 0.0 && first < 0.2, "trails: {first}");
        let mut h = first;
        for _ in 0..200 {
            h = s.update(1.0, 0.016);
        }
        assert!((h - 1.0).abs() < 1e-3, "settles: {h}");
        // From just under +π to just over −π is a small turn, not a full spin.
        let mut w = ChaseSmoothing::default();
        w.update(3.1, 0.016);
        let next = w.update(-3.1, 0.016);
        assert!(next > 3.1, "goes the short way: {next}");
    }
}
