//! Cameras (chase, cab, top, side, showcase) and lighting/atmosphere settings.

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

    /// [`Camera::view_proj`] with the eye at the origin: the renderer draws
    /// everything relative to the camera (a floating origin), so positions
    /// kilometres from the world origin keep millimetre precision on the GPU.
    pub fn view_proj_relative(&self, aspect: f32) -> Mat4 {
        let view =
            glam::camera::rh::view::look_at_mat4(Vec3::ZERO, self.target - self.eye, self.up);
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
    /// Weather fog at ground level, extinction per m (3 / visibility); 0:
    /// none. It thins with height, so the sky overhead stays lighter.
    pub ground_fog: f32,
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
            ground_fog: 0.0,
            exposure: 1.0,
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Environment {
    /// Clear-sky light at `hour` (0–24): the sun rises in the east at 6,
    /// stands 75° high in the south at noon and sets in the west at 18;
    /// golden light near the horizon, a dark blue night, and the eye's
    /// exposure opening up after dusk.
    pub fn at_hour(hour: f32) -> Self {
        Self::at_hour_from(Environment::default(), hour)
    }

    /// Daylight colours from a game's look (sky, ground, haze, exposure).
    pub fn from_look(look: &wp_world::Look) -> Self {
        let base = Environment::default();
        Environment {
            sky_zenith: Vec3::from(look.sky_top),
            sky_horizon: Vec3::from(look.sky_horizon),
            ground_base: Vec3::from(look.ground),
            ground_line: Vec3::from(look.ground_lines),
            fog_density: base.fog_density * look.fog,
            // Aerial haze: about 1200 m of visibility at haze 1.
            ground_fog: look.haze * 0.0025,
            exposure: base.exposure * look.exposure,
            ..base
        }
    }

    /// [`Environment::at_hour`] with `base` as the daylight colours.
    pub fn at_hour_from(base: Environment, hour: f32) -> Self {
        let day = (hour.rem_euclid(24.0) - 6.0) / 12.0;
        let elevation = 75f32.to_radians() * (std::f32::consts::PI * day).sin();
        let azimuth = -std::f32::consts::PI * day;
        let sun_dir = Vec3::new(
            azimuth.cos() * elevation.cos(),
            azimuth.sin() * elevation.cos(),
            elevation.sin(),
        );
        let s = sun_dir.z;
        let daylight = smoothstep(-0.10, 0.20, s);
        let golden = 1.0 - smoothstep(0.0, 0.35, s.abs());
        let sunset = Vec3::new(0.85, 0.5, 0.32);
        let horizon = Vec3::new(0.012, 0.016, 0.03).lerp(base.sky_horizon, daylight);
        Environment {
            sun_dir,
            sun_intensity: base.sun_intensity * smoothstep(-0.02, 0.25, s),
            sun_color: base.sun_color.lerp(Vec3::new(1.0, 0.55, 0.3), golden),
            sky_zenith: Vec3::new(0.004, 0.006, 0.016).lerp(base.sky_zenith, daylight),
            sky_horizon: horizon.lerp(sunset, golden * daylight * 0.6),
            exposure: base.exposure * (1.0 + 1.0 * (1.0 - daylight)),
            ..base
        }
    }

    /// 0 in daylight to 1 at night (from the sun's height).
    pub fn darkness(&self) -> f32 {
        1.0 - smoothstep(-0.06, 0.12, self.sun_dir.z)
    }

    /// The same light under rain `r` (0–1): overcast sky, weaker sun, haze.
    pub fn with_rain(mut self, r: f32) -> Self {
        let r = r.clamp(0.0, 1.0);
        // Cloud is grey by day; at night it is dark, lit faintly from below
        // by the town (warmer at the horizon).
        let light = 1.0 - self.darkness();
        let grey_zenith = Vec3::new(0.008, 0.009, 0.012).lerp(Vec3::new(0.36, 0.39, 0.44), light);
        let grey_horizon = Vec3::new(0.03, 0.026, 0.022).lerp(Vec3::new(0.55, 0.57, 0.6), light);
        self.sky_zenith = self.sky_zenith.lerp(grey_zenith, r * 0.9);
        self.sky_horizon = self.sky_horizon.lerp(grey_horizon, r * 0.9);
        self.sun_intensity *= 1.0 - 0.75 * r;
        self.sun_color = self.sun_color.lerp(Vec3::new(0.85, 0.88, 0.92), r);
        self.fog_density *= 1.0 + 3.0 * r;
        self.exposure *= 1.0 + 0.35 * r;
        self
    }

    /// The same light in fog `amount` (0–1) that one can see `visibility_m`
    /// through: a grey sky, a sun dimmed to a glow, light from everywhere.
    pub fn with_fog(mut self, amount: f32, visibility_m: Option<f32>) -> Self {
        let Some(sight) = visibility_m else {
            return self;
        };
        let f = amount.clamp(0.0, 1.0);
        let light = 1.0 - self.darkness();
        // Lit by day; at night faintly warm from the town's lights.
        let grey = Vec3::new(0.03, 0.027, 0.024).lerp(Vec3::new(0.6, 0.62, 0.64), light);
        self.sky_zenith = self.sky_zenith.lerp(grey * 0.9, f * 0.85);
        self.sky_horizon = self.sky_horizon.lerp(grey, f * 0.95);
        self.sun_intensity *= 1.0 - 0.8 * f;
        self.sun_color = self.sun_color.lerp(Vec3::new(0.95, 0.93, 0.9), f);
        // Fog is defined by visibility: contrast falls to 5 % at `sight`.
        self.ground_fog = self.ground_fog.max(3.0 / sight.max(1.0));
        self.exposure *= 1.0 + 0.25 * f;
        self
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
    /// The driver's seat, looking out of the windscreen.
    Cab,
}

impl CameraMode {
    pub const ALL: [(&'static str, CameraMode); 5] = [
        ("chase", CameraMode::Chase),
        ("cab", CameraMode::Cab),
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
            // Without a vehicle to sit in: eye height, looking ahead.
            CameraMode::Cab => (pos + Vec3::Z * 1.2, pos + fwd * 10.0 + Vec3::Z * 1.0),
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

#[cfg(test)]
mod sky_tests {
    use super::*;

    #[test]
    fn the_sun_follows_the_clock() {
        let noon = Environment::at_hour(12.0);
        assert!(
            noon.sun_dir.z > 0.95 && noon.sun_dir.y < 0.0,
            "high in the south"
        );
        assert_eq!(noon.darkness(), 0.0);
        let morning = Environment::at_hour(8.0);
        assert!(morning.sun_dir.x > 0.5, "east in the morning");
        let evening = Environment::at_hour(17.5);
        assert!(evening.sun_dir.x < -0.5, "west in the evening");
        assert!(
            evening.sun_color.z < noon.sun_color.z,
            "warmer light near sunset"
        );
        let midnight = Environment::at_hour(0.0);
        assert_eq!(midnight.darkness(), 1.0);
        assert!(midnight.exposure > noon.exposure);
    }
}
