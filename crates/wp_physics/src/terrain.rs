//! Terrain: ground height on a regular grid. Physics (slopes), rendering
//! (the land mesh, where things stand) and gameplay all read the same grid,
//! so what is drawn is what is driven on. Heights between grid points are
//! bilinear; outside the grid the ground is flat at height 0.

use glam::Vec2;
use std::sync::Arc;

/// Heights on a grid of `nx` × `ny` points, `cell_m` apart, starting at
/// `origin` (the south-west corner, world metres).
#[derive(Clone, Debug, PartialEq)]
pub struct HeightField {
    pub origin: Vec2,
    pub cell_m: f32,
    pub nx: usize,
    pub ny: usize,
    /// Row-major, x fastest: `heights[j * nx + i]` is the point `(i, j)`.
    pub heights: Vec<f32>,
}

impl HeightField {
    /// A flat field (every height 0).
    pub fn flat(origin: Vec2, cell_m: f32, nx: usize, ny: usize) -> Self {
        HeightField {
            origin,
            cell_m,
            nx,
            ny,
            heights: vec![0.0; nx * ny],
        }
    }

    /// Height at grid point `(i, j)` (clamped to the grid).
    pub fn at(&self, i: usize, j: usize) -> f32 {
        self.heights[j.min(self.ny - 1) * self.nx + i.min(self.nx - 1)]
    }

    /// World position of grid point `(i, j)`.
    pub fn point(&self, i: usize, j: usize) -> Vec2 {
        self.origin + Vec2::new(i as f32, j as f32) * self.cell_m
    }

    /// The far (north-east) corner.
    pub fn max(&self) -> Vec2 {
        self.point(self.nx - 1, self.ny - 1)
    }

    /// Cell and position inside it (0–1), or `None` outside the grid.
    fn locate(&self, p: Vec2) -> Option<(usize, usize, f32, f32)> {
        if self.nx < 2 || self.ny < 2 {
            return None;
        }
        let q = (p - self.origin) / self.cell_m;
        let (w, h) = ((self.nx - 1) as f32, (self.ny - 1) as f32);
        if !(q.x >= 0.0 && q.y >= 0.0 && q.x <= w && q.y <= h) {
            return None;
        }
        let i = (q.x.floor() as usize).min(self.nx - 2);
        let j = (q.y.floor() as usize).min(self.ny - 2);
        Some((i, j, q.x - i as f32, q.y - j as f32))
    }

    /// Ground height at `p`, m.
    pub fn height(&self, p: Vec2) -> f32 {
        match self.locate(p) {
            Some((i, j, u, v)) => {
                let (a, b) = (self.at(i, j), self.at(i + 1, j));
                let (c, d) = (self.at(i, j + 1), self.at(i + 1, j + 1));
                let low = a + (b - a) * u;
                let high = c + (d - c) * u;
                low + (high - low) * v
            }
            None => 0.0,
        }
    }

    /// Slope at `p`: (dh/dx, dh/dy), rise per metre.
    pub fn gradient(&self, p: Vec2) -> Vec2 {
        match self.locate(p) {
            Some((i, j, u, v)) => {
                let (a, b) = (self.at(i, j), self.at(i + 1, j));
                let (c, d) = (self.at(i, j + 1), self.at(i + 1, j + 1));
                let dx = (b - a) + ((d - c) - (b - a)) * v;
                let dy = (c - a) + ((d - b) - (c - a)) * u;
                Vec2::new(dx, dy) / self.cell_m
            }
            None => Vec2::ZERO,
        }
    }

    /// Lowest and highest heights.
    pub fn range(&self) -> (f32, f32) {
        self.heights
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), h| (lo.min(*h), hi.max(*h)))
    }
}

/// The scene's ground shape (absent: flat ground at height 0).
#[derive(Clone, Debug)]
pub struct Terrain(pub Arc<HeightField>);

impl Terrain {
    pub fn height(&self, p: Vec2) -> f32 {
        self.0.height(p)
    }
    pub fn gradient(&self, p: Vec2) -> Vec2 {
        self.0.gradient(p)
    }
}

/// Standing water on the terrain's grid: the surface height over each
/// grid point under water (NaN where dry). Lakes can sit at any height.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterLevels {
    pub origin: Vec2,
    pub cell_m: f32,
    pub nx: usize,
    pub ny: usize,
    pub levels: Vec<f32>,
}

impl WaterLevels {
    /// Surface height of the water nearest `p`, if it is under water.
    pub fn level_at(&self, p: Vec2) -> Option<f32> {
        let q = ((p - self.origin) / self.cell_m).round();
        if q.x < 0.0 || q.y < 0.0 {
            return None;
        }
        let (i, j) = (q.x as usize, q.y as usize);
        if i >= self.nx || j >= self.ny {
            return None;
        }
        let l = self.levels[j * self.nx + i];
        (!l.is_nan()).then_some(l)
    }
}

/// The scene's standing water (lakes, ponds, fords).
#[derive(Clone, Debug)]
pub struct Water(pub Arc<WaterLevels>);

impl Water {
    /// Water depth at `p` on `field`, m (0 on dry land).
    pub fn depth(&self, field: &HeightField, p: Vec2) -> f32 {
        self.0
            .level_at(p)
            .map_or(0.0, |l| (l - field.height(p)).max(0.0))
    }
}

/// How a body sits on the ground: height under its centre and the slope
/// along and across it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Footing {
    /// Ground height under the centre, m.
    pub height_m: f32,
    /// Rise per metre forward (nose up positive) and to the left (left side
    /// up positive), measured between the body's contact points.
    pub grade: Vec2,
}

impl Footing {
    /// Sine of the nose-up pitch and of the left-up roll the ground imposes.
    pub fn sines(&self) -> Vec2 {
        let s = |t: f32| t / (1.0 + t * t).sqrt();
        Vec2::new(s(self.grade.x), s(self.grade.y))
    }

    /// Share of the weight pressing into the ground (cosine of the tilt).
    pub fn normal_share(&self) -> f32 {
        1.0 / (1.0 + self.grade.length_squared()).sqrt()
    }

    /// Footing of a body at `position` (heading `heading`, rad) on four
    /// contact points `front`/`rear` m ahead/behind the centre and
    /// `half_width` either side.
    pub fn on(
        field: &HeightField,
        position: Vec2,
        heading: f32,
        front: f32,
        rear: f32,
        half_width: f32,
    ) -> Footing {
        let f = Vec2::from_angle(heading);
        let l = f.perp();
        let h = |x: f32, y: f32| field.height(position + f * x + l * y);
        let (fl, fr) = (h(front, half_width), h(front, -half_width));
        let (rl, rr) = (h(-rear, half_width), h(-rear, -half_width));
        let length = (front + rear).max(0.1);
        let width = (2.0 * half_width).max(0.1);
        Footing {
            height_m: field.height(position),
            grade: Vec2::new(
                ((fl + fr) - (rl + rr)) * 0.5 / length,
                ((fl + rl) - (fr + rr)) * 0.5 / width,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> HeightField {
        // Rises 1 m per 10 m eastward.
        let mut f = HeightField::flat(Vec2::new(-50.0, -50.0), 5.0, 21, 21);
        for j in 0..21 {
            for i in 0..21 {
                f.heights[j * 21 + i] = 0.1 * (i as f32 * 5.0);
            }
        }
        f
    }

    #[test]
    fn bilinear_heights_and_slopes() {
        let f = ramp();
        assert!((f.height(Vec2::new(0.0, 3.0)) - 5.0).abs() < 1e-4);
        assert!((f.height(Vec2::new(12.5, -7.0)) - 6.25).abs() < 1e-4);
        let g = f.gradient(Vec2::new(1.0, 1.0));
        assert!((g.x - 0.1).abs() < 1e-5 && g.y.abs() < 1e-6);
        // Outside the grid: flat at 0.
        assert_eq!(f.height(Vec2::new(500.0, 0.0)), 0.0);
        assert_eq!(f.gradient(Vec2::new(500.0, 0.0)), Vec2::ZERO);
    }

    #[test]
    fn footing_follows_heading() {
        let f = ramp();
        // Facing east: nose up 10 %, level side to side.
        let east = Footing::on(&f, Vec2::ZERO, 0.0, 1.5, 1.3, 0.8);
        assert!((east.grade.x - 0.1).abs() < 1e-4 && east.grade.y.abs() < 1e-4);
        // Facing north: the right side (east) is higher, so the left is down.
        let north = Footing::on(&f, Vec2::ZERO, std::f32::consts::FRAC_PI_2, 1.5, 1.3, 0.8);
        assert!(north.grade.x.abs() < 1e-4 && (north.grade.y + 0.1).abs() < 1e-4);
        assert!((east.sines().x - 0.1 / 1.01f32.sqrt()).abs() < 1e-5);
    }
}
