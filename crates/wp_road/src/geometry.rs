//! Polylines and road centreline shapes. Pure geometry, no world access.

use crate::def::RoadShape;
use glam::Vec2;

/// Points along a path with cumulative arc length.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polyline {
    pub points: Vec<Vec2>,
    /// `lengths[i]` = distance along the path to `points[i]`.
    pub lengths: Vec<f32>,
}

impl Polyline {
    pub fn new(points: Vec<Vec2>) -> Self {
        let mut pts: Vec<Vec2> = Vec::with_capacity(points.len());
        for p in points {
            if pts.last().is_none_or(|q: &Vec2| q.distance(p) > 1e-4) {
                pts.push(p);
            }
        }
        let mut lengths = Vec::with_capacity(pts.len());
        let mut acc = 0.0;
        for i in 0..pts.len() {
            if i > 0 {
                acc += pts[i].distance(pts[i - 1]);
            }
            lengths.push(acc);
        }
        Self {
            points: pts,
            lengths,
        }
    }

    pub fn length(&self) -> f32 {
        self.lengths.last().copied().unwrap_or(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.points.len() < 2
    }

    /// Unit direction at vertex `i` (average of its neighbouring segments).
    pub fn tangent_at(&self, i: usize) -> Vec2 {
        let n = self.points.len();
        if n < 2 {
            return Vec2::X;
        }
        let a = self.points[i.saturating_sub(1)];
        let b = self.points[(i + 1).min(n - 1)];
        let d = if i == 0 {
            self.points[1] - self.points[0]
        } else if i == n - 1 {
            self.points[n - 1] - self.points[n - 2]
        } else {
            b - a
        };
        d.normalize_or(Vec2::X)
    }

    pub fn start_direction(&self) -> Vec2 {
        self.tangent_at(0)
    }

    pub fn end_direction(&self) -> Vec2 {
        self.tangent_at(self.points.len().saturating_sub(1))
    }

    /// Position and unit direction at arc length `s` (clamped to the path).
    pub fn sample(&self, s: f32) -> (Vec2, Vec2) {
        if self.points.len() < 2 {
            return (self.points.first().copied().unwrap_or_default(), Vec2::X);
        }
        let s = s.clamp(0.0, self.length());
        let i = self
            .lengths
            .partition_point(|&l| l < s)
            .clamp(1, self.points.len() - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let seg = self.lengths[i] - self.lengths[i - 1];
        let u = if seg > 0.0 {
            (s - self.lengths[i - 1]) / seg
        } else {
            0.0
        };
        (a.lerp(b, u), (b - a).normalize_or(Vec2::X))
    }

    /// The path shifted sideways by `d` (positive = left), joining segments
    /// with mitred corners (limited so sharp corners do not spike).
    pub fn offset(&self, d: f32) -> Polyline {
        self.offset_with(|_| d)
    }

    /// The path shifted sideways by `d(s)` at arc length `s` (positive = left),
    /// e.g. a lane that narrows or moves over. Sample the path finely first.
    pub fn offset_with(&self, d_at: impl Fn(f32) -> f32) -> Polyline {
        let n = self.points.len();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let d = d_at(self.lengths[i]);
            let normal = |t: Vec2| Vec2::new(-t.y, t.x);
            let p = self.points[i];
            let shift = if i == 0 || i == n - 1 {
                normal(self.tangent_at(i)) * d
            } else {
                let t0 = (self.points[i] - self.points[i - 1]).normalize_or(Vec2::X);
                let t1 = (self.points[i + 1] - self.points[i]).normalize_or(Vec2::X);
                let m = (normal(t0) + normal(t1)).normalize_or(normal(t0));
                let cos = m.dot(normal(t0)).max(0.3);
                m * (d / cos)
            };
            out.push(p + shift);
        }
        Polyline::new(out)
    }

    /// The part between arc lengths `start` and `end` (both clamped).
    pub fn slice(&self, start: f32, end: f32) -> Polyline {
        let (start, end) = (start.max(0.0), end.min(self.length()));
        if end <= start {
            return Polyline::default();
        }
        let mut pts = vec![self.sample(start).0];
        for (p, &l) in self.points.iter().zip(&self.lengths) {
            if l > start && l < end {
                pts.push(*p);
            }
        }
        pts.push(self.sample(end).0);
        Polyline::new(pts)
    }

    /// Remove `from_start` metres at the start and `from_end` at the end.
    pub fn trim(&self, from_start: f32, from_end: f32) -> Polyline {
        self.slice(from_start, self.length() - from_end)
    }

    pub fn reversed(&self) -> Polyline {
        Polyline::new(self.points.iter().rev().copied().collect())
    }

    /// Evenly re-spaced points, about `step` metres apart.
    pub fn resample(&self, step: f32) -> Polyline {
        let len = self.length();
        let n = ((len / step).ceil() as usize).max(1);
        Polyline::new(
            (0..=n)
                .map(|k| self.sample(len * k as f32 / n as f32).0)
                .collect(),
        )
    }

    /// Closest point: (arc length, signed lateral offset (+ left), distance).
    pub fn project(&self, p: Vec2) -> (f32, f32, f32) {
        self.project_between(p, f32::NEG_INFINITY, f32::INFINITY)
    }

    /// Like [`Polyline::project`], but only onto the part between arc lengths
    /// `from` and `to`. Followers use this to track progress, so a path that
    /// passes the same place twice does not make them jump ahead.
    pub fn project_between(&self, p: Vec2, from: f32, to: f32) -> (f32, f32, f32) {
        let mut best = (from.max(0.0), 0.0, f32::INFINITY);
        for i in 1..self.points.len() {
            if self.lengths[i] < from || self.lengths[i - 1] > to {
                continue;
            }
            let (a, b) = (self.points[i - 1], self.points[i]);
            let ab = b - a;
            let len2 = ab.length_squared();
            let u = if len2 > 0.0 {
                ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = a + ab * u;
            let dist = q.distance(p);
            if dist < best.2 {
                let side = ab.perp_dot(p - a).signum();
                best = (self.lengths[i - 1] + ab.length() * u, side * dist, dist);
            }
        }
        best
    }

    /// Join paths end to start (skipping the duplicate joint).
    pub fn concat(parts: &[&Polyline]) -> Polyline {
        let mut pts = Vec::new();
        for part in parts {
            pts.extend_from_slice(&part.points);
        }
        Polyline::new(pts)
    }
}

/// Centreline from `a` to `b` for a road shape, sampled about every 2 m.
pub fn centreline(shape: &RoadShape, a: Vec2, b: Vec2) -> Polyline {
    let raw = match shape {
        RoadShape::Straight => Polyline::new(vec![a, b]),
        RoadShape::Arc { radius_m } => arc(a, b, *radius_m),
        RoadShape::Curve { through } => {
            let mut ctrl = vec![a];
            ctrl.extend(through.iter().map(|p| Vec2::new(p.0, p.1)));
            ctrl.push(b);
            catmull_rom(&ctrl, 12)
        }
        RoadShape::Polyline { points } => {
            let mut pts = vec![a];
            pts.extend(points.iter().map(|p| Vec2::new(p.0, p.1)));
            pts.push(b);
            return Polyline::new(pts);
        }
    };
    raw.resample(2.0)
}

/// The shorter circular arc from `a` to `b`; positive radius bends left.
fn arc(a: Vec2, b: Vec2, radius: f32) -> Polyline {
    let chord = b - a;
    let c = chord.length() / 2.0;
    let r = radius.abs().max(c + 1e-3);
    let dir = chord / (2.0 * c);
    let left = Vec2::new(-dir.y, dir.x);
    let side = radius.signum();
    let centre = a + chord / 2.0 + left * side * (r * r - c * c).sqrt();
    let a0 = (a - centre).to_angle();
    let mut sweep = (b - centre).to_angle() - a0;
    // Left bends turn counter-clockwise, right bends clockwise; take the minor arc.
    let pi = std::f32::consts::PI;
    while sweep > pi {
        sweep -= 2.0 * pi;
    }
    while sweep < -pi {
        sweep += 2.0 * pi;
    }
    let n = ((sweep.abs() * r / 1.0).ceil() as usize).max(4);
    Polyline::new(
        (0..=n)
            .map(|k| centre + Vec2::from_angle(a0 + sweep * k as f32 / n as f32) * r)
            .collect(),
    )
}

/// Uniform Catmull-Rom spline through `ctrl`, `per_segment` samples per span.
pub fn catmull_rom(ctrl: &[Vec2], per_segment: usize) -> Polyline {
    if ctrl.len() < 3 {
        return Polyline::new(ctrl.to_vec());
    }
    let get = |i: isize| ctrl[i.clamp(0, ctrl.len() as isize - 1) as usize];
    let mut pts = Vec::new();
    for i in 0..ctrl.len() - 1 {
        let (p0, p1, p2, p3) = (
            get(i as isize - 1),
            get(i as isize),
            get(i as isize + 1),
            get(i as isize + 2),
        );
        for k in 0..per_segment {
            let t = k as f32 / per_segment as f32;
            let (t2, t3) = (t * t, t * t * t);
            pts.push(
                0.5 * ((2.0 * p1)
                    + (-p0 + p2) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3),
            );
        }
    }
    pts.push(*ctrl.last().unwrap());
    Polyline::new(pts)
}

/// Cubic Bézier from `p0` heading `t0` to `p3` arriving along `t3`.
pub fn connector(p0: Vec2, t0: Vec2, p3: Vec2, t3: Vec2, samples: usize) -> Polyline {
    let d = p0.distance(p3) * 0.4;
    let (p1, p2) = (p0 + t0 * d, p3 - t3 * d);
    Polyline::new(
        (0..=samples)
            .map(|k| {
                let t = k as f32 / samples as f32;
                let u = 1.0 - t;
                p0 * u * u * u + p1 * 3.0 * u * u * t + p2 * 3.0 * u * t * t + p3 * t * t * t
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_line_length_and_sampling() {
        let p = centreline(&RoadShape::Straight, Vec2::ZERO, Vec2::new(10.0, 0.0));
        assert!((p.length() - 10.0).abs() < 1e-4);
        let (pos, dir) = p.sample(2.5);
        assert!((pos - Vec2::new(2.5, 0.0)).length() < 1e-4 && (dir - Vec2::X).length() < 1e-4);
    }

    #[test]
    fn arc_bends_the_right_way_with_the_right_radius() {
        let a = Vec2::ZERO;
        let b = Vec2::new(20.0, 20.0);
        let left = centreline(&RoadShape::Arc { radius_m: 20.0 }, a, b);
        // A left quarter circle from (0,0) to (20,20) has its centre at (0,20)... measured from the centre:
        let mid = left.sample(left.length() / 2.0).0;
        assert!(
            mid.x > 10.0 && mid.y < 10.0,
            "left bend should bulge right of the chord: {mid}"
        );
        assert!(
            (left.length() - std::f32::consts::FRAC_PI_2 * 20.0).abs() < 0.3,
            "len {}",
            left.length()
        );
    }

    #[test]
    fn offset_trim_project_round_trip() {
        let p = centreline(&RoadShape::Straight, Vec2::ZERO, Vec2::new(30.0, 0.0));
        let l = p.offset(1.75);
        assert!((l.points[0].y - 1.75).abs() < 1e-4);
        let t = p.trim(5.0, 5.0);
        assert!((t.length() - 20.0).abs() < 1e-3);
        let (s, lat, _) = p.project(Vec2::new(12.0, -2.0));
        assert!((s - 12.0).abs() < 1e-3 && (lat + 2.0).abs() < 1e-3);
    }

    #[test]
    fn connector_meets_both_ends_tangentially() {
        let c = connector(Vec2::ZERO, Vec2::X, Vec2::new(10.0, 10.0), Vec2::Y, 200);
        assert!((c.start_direction() - Vec2::X).length() < 0.05);
        assert!((c.end_direction() - Vec2::Y).length() < 0.05);
    }
}
