//! Rain on the windscreen in the cab view, and the wipers that clear it.
//!
//! The glass is a rectangle in front of the driver. Drops are drawn by the
//! shader (`fs_glass`) from a hash of where they sit, so nothing is stored
//! between frames: a drop shows once enough time has passed since a blade
//! last crossed it. The wipers' motion is a pure function of simulated time,
//! and [`Wipers::since_wiped`] (mirrored in the shader) finds when a blade
//! last passed a point. Same time, same picture.

use super::DrawItem;
use super::MeshId;
use glam::{Mat4, Vec2, Vec3};

/// Angle of a parked blade above the glass's bottom edge, rad.
pub const PARK_RAD: f32 = 0.04;
/// Angle at the top of a sweep, rad (about 112°).
pub const TOP_RAD: f32 = 1.95;
/// Blades clear from this share of the arm's length to its tip.
pub const BLADE_FROM: f32 = 0.3;

/// How fast the wipers run, from how hard it rains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WiperSpeed {
    Off,
    /// One sweep every few seconds.
    Intermittent,
    Normal,
    Fast,
}

impl WiperSpeed {
    /// What a driver would pick for rain `r` (0–1).
    pub fn for_rain(r: f32) -> Self {
        match r {
            r if r <= 0.01 => Self::Off,
            r if r < 0.3 => Self::Intermittent,
            r if r < 0.65 => Self::Normal,
            _ => Self::Fast,
        }
    }

    /// (one cycle, one sweep up and back), s; `None` when off.
    pub fn timing(self) -> Option<(f32, f32)> {
        match self {
            Self::Off => None,
            Self::Intermittent => Some((5.0, 1.35)),
            Self::Normal => Some((1.35, 1.35)),
            Self::Fast => Some((0.95, 0.95)),
        }
    }
}

/// Two wipers on parallel arms (a tandem system). Positions are on the
/// glass in metres from its bottom-left corner as the driver sees it; when
/// `mirrored` (driver on the right) they are measured from the right instead,
/// so the blades park pointing left and sweep up towards the driver.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wipers {
    pub pivots: [Vec2; 2],
    pub arm_m: f32,
    pub mirrored: bool,
    pub speed: WiperSpeed,
}

impl Wipers {
    /// Wipers sized for a glass `width` × `height` m.
    pub fn for_glass(width: f32, height: f32, mirrored: bool, speed: WiperSpeed) -> Self {
        Wipers {
            pivots: [Vec2::new(0.2 * width, 0.02), Vec2::new(0.62 * width, 0.02)],
            arm_m: 0.9 * height,
            mirrored,
            speed,
        }
    }

    /// Arm angle at time `t` (s), rad from the parked direction's baseline.
    /// Each sweep eases up and back like a crank-driven linkage.
    pub fn angle(&self, t: f32) -> f32 {
        let Some((cycle, stroke)) = self.speed.timing() else {
            return PARK_RAD;
        };
        let tau = t.rem_euclid(cycle);
        if tau >= stroke {
            return PARK_RAD;
        }
        let k = (1.0 - (std::f32::consts::TAU * tau / stroke).cos()) * 0.5;
        PARK_RAD + (TOP_RAD - PARK_RAD) * k
    }

    /// From glass coordinates to the wipers' own (unmirrored) frame.
    fn local(&self, p: Vec2, width: f32) -> Vec2 {
        if self.mirrored {
            Vec2::new(width - p.x, p.y)
        } else {
            p
        }
    }

    /// Seconds since a blade last crossed `p` (glass coordinates, m) at
    /// time `t`, and a number for that pass (different for every pass, so
    /// each wipe leaves a fresh set of drops). `None`: never wiped there.
    pub fn since_wiped(&self, p: Vec2, width: f32, t: f32) -> Option<(f32, i64)> {
        let (cycle, stroke) = self.speed.timing()?;
        let q = self.local(p, width);
        let n = (t / cycle).floor();
        let tau = t - n * cycle;
        let mut best: Option<(f32, i64)> = None;
        for (k, pivot) in self.pivots.iter().enumerate() {
            let d = q - *pivot;
            let r = d.length();
            if r < self.arm_m * BLADE_FROM || r > self.arm_m {
                continue;
            }
            let f = (d.y.atan2(d.x) - PARK_RAD) / (TOP_RAD - PARK_RAD);
            if !(0.0..=1.0).contains(&f) {
                continue;
            }
            // The blade reaches this angle going up at `up`, coming back at `down`.
            let up = stroke / std::f32::consts::TAU * (1.0 - 2.0 * f).acos();
            let down = stroke - up;
            let (at, pass) = if tau >= down {
                (n * cycle + down, n * 2.0 + 1.0)
            } else if tau >= up {
                (n * cycle + up, n * 2.0)
            } else {
                ((n - 1.0) * cycle + down, n * 2.0 - 1.0)
            };
            let since = t - at;
            let id = pass as i64 * 2 + k as i64;
            if best.is_none_or(|b| since < b.0) {
                best = Some((since, id));
            }
        }
        best
    }
}

/// The windscreen in the world, for the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Windscreen {
    /// Bottom-left corner of the glass as the driver sees it.
    pub origin: Vec3,
    /// Unit vectors along the glass: left to right, and bottom to top.
    pub across: Vec3,
    pub up: Vec3,
    pub width_m: f32,
    pub height_m: f32,
    /// How hard it rains, 0–1 (how many drops, how fast they gather).
    pub rain: f32,
    /// Simulated time, s (drives the wipers and the drops).
    pub time_s: f32,
    pub wipers: Wipers,
}

/// The glass in the body frame (x forward, y left, z up), from where the
/// cab's dashboard, pillars and roof edge sit around eye point `eye`:
/// (bottom-left corner, width, height).
pub fn glass(half_width: f32, eye: Vec3) -> (Vec3, f32, f32) {
    let dash_top = eye.z - 0.33;
    let x = eye.x + 1.09;
    let side = half_width - 0.1;
    (
        Vec3::new(x, side, dash_top),
        side * 2.0,
        eye.z + 0.28 - dash_top,
    )
}

/// The wiper arms and blades, outside the glass at time `t`.
pub(super) fn wiper_items(
    items: &mut Vec<DrawItem>,
    body: Mat4,
    glass: (Vec3, f32, f32),
    w: &Wipers,
    t: f32,
) {
    let (origin, width, _) = glass;
    // Body frame: across the glass runs to the right (−y), up is +z, and
    // the outside is +x (the box's depth axis points in, to stay right-handed).
    let across = Vec3::NEG_Y;
    let up = Vec3::Z;
    let a = w.angle(t);
    let dir = Vec2::new(if w.mirrored { -a.cos() } else { a.cos() }, a.sin());
    let along = across * dir.x + up * dir.y;
    let side = across * -dir.y + up * dir.x;
    let black = Vec3::splat(0.018);
    for pivot in w.pivots {
        let p = Vec2::new(if w.mirrored { width - pivot.x } else { pivot.x }, pivot.y);
        let base = origin + across * p.x + up * p.y;
        let mut part = |from: f32, to: f32, wide: f32, out: f32, deep: f32| {
            let centre = base + along * (w.arm_m * (from + to) * 0.5) + Vec3::X * out;
            items.push(DrawItem {
                mesh: MeshId::Cube,
                transform: body
                    * Mat4::from_cols(
                        (along * w.arm_m * (to - from)).extend(0.0),
                        (side * wide).extend(0.0),
                        (Vec3::NEG_X * deep).extend(0.0),
                        centre.extend(1.0),
                    ),
                color: black,
                roughness: 0.7,
                metallic: 0.0,
                emissive: 0.0,
            });
        };
        // The arm rides a little off the glass; the blade lies on it.
        part(0.0, 1.0, 0.012, 0.035, 0.012);
        part(BLADE_FROM, 1.0, 0.02, 0.014, 0.016);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car(speed: WiperSpeed, mirrored: bool) -> Wipers {
        Wipers::for_glass(1.4, 0.61, mirrored, speed)
    }

    #[test]
    fn speed_follows_the_rain() {
        assert_eq!(WiperSpeed::for_rain(0.0), WiperSpeed::Off);
        assert_eq!(WiperSpeed::for_rain(0.15), WiperSpeed::Intermittent);
        assert_eq!(WiperSpeed::for_rain(0.5), WiperSpeed::Normal);
        assert_eq!(WiperSpeed::for_rain(0.9), WiperSpeed::Fast);
    }

    #[test]
    fn wipers_sweep_up_and_back_then_park() {
        let w = car(WiperSpeed::Intermittent, false);
        assert!((w.angle(0.0) - PARK_RAD).abs() < 1e-5);
        assert!((w.angle(1.35 / 2.0) - TOP_RAD).abs() < 1e-4);
        // Between sweeps they rest.
        assert_eq!(w.angle(3.0), PARK_RAD);
        assert_eq!(car(WiperSpeed::Off, false).angle(0.7), PARK_RAD);
    }

    /// The blade really was at the point when `since_wiped` says, and has not
    /// crossed it since.
    #[test]
    fn last_wipe_matches_the_blade() {
        for mirrored in [false, true] {
            let w = car(WiperSpeed::Normal, mirrored);
            for (i, p) in [
                Vec2::new(0.6, 0.3),
                Vec2::new(0.2, 0.45),
                Vec2::new(1.1, 0.2),
                Vec2::new(0.9, 0.5),
            ]
            .into_iter()
            .enumerate()
            {
                let t = 7.3 + i as f32 * 0.41;
                let (since, _) = w.since_wiped(p, 1.4, t).expect("in a sweep");
                let q = w.local(p, 1.4);
                // Which blade: the one that was over the point then.
                let at = t - since;
                let hit = w.pivots.iter().any(|pv| {
                    let d = q - *pv;
                    (d.y.atan2(d.x) - w.angle(at)).abs() < 2e-3
                });
                assert!(hit, "{p:?} at {at}");
                // No blade crossed it between then and now.
                let steps = 200;
                for s in 1..steps {
                    let u = at + since * s as f32 / steps as f32;
                    let crossed = w.pivots.iter().any(|pv| {
                        let d = q - *pv;
                        d.length() >= w.arm_m * BLADE_FROM
                            && d.length() <= w.arm_m
                            && (d.y.atan2(d.x) - w.angle(u)).abs() < 1e-4
                    });
                    assert!(!crossed, "{p:?} crossed again at {u}");
                }
            }
        }
    }

    #[test]
    fn corners_out_of_reach_are_never_wiped() {
        let w = car(WiperSpeed::Fast, false);
        // Bottom right corner: below every arm's sweep.
        assert!(w.since_wiped(Vec2::new(1.39, 0.005), 1.4, 3.0).is_none());
        // Right by a pivot: inside the blade's inner end.
        assert!(w.since_wiped(Vec2::new(0.3, 0.06), 1.4, 3.0).is_none());
        assert!(
            car(WiperSpeed::Off, false)
                .since_wiped(Vec2::new(0.6, 0.3), 1.4, 3.0)
                .is_none()
        );
    }

    #[test]
    fn each_pass_gets_its_own_number() {
        let w = car(WiperSpeed::Normal, false);
        let p = Vec2::new(0.6, 0.3);
        let ids: Vec<i64> = (0..40)
            .map(|i| w.since_wiped(p, 1.4, 5.0 + i as f32 * 0.1).unwrap().1)
            .collect();
        let changes = ids.windows(2).filter(|x| x[0] != x[1]).count();
        // 4 s at 1.35 s per cycle: about six passes (up and back).
        assert!((4..=8).contains(&changes), "{changes}: {ids:?}");
    }
}
