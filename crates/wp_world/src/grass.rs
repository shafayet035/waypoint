//! Grass: dense tufts near the camera that sway in the wind. A scene's
//! `grass` settings say how thick, tall and green it is; [`build`] works out
//! where it may grow (a mask on a 1 m grid: not on roads, sidewalks,
//! junctions or in buildings, thinning out on steep ground). The renderer
//! places the tufts itself, on the GPU, from that mask.

use glam::Vec2;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wp_physics::HeightField;
use wp_road::RoadNetwork;

/// A scene's grass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrassDef {
    /// Tufts per square metre close to the camera (it thins out further away).
    #[serde(default = "default_density")]
    pub density_per_m2: f32,
    /// Height of a typical tuft, m.
    #[serde(default = "default_height")]
    pub height_m: f32,
    /// Colour at the roots and at the tips (sRGB 0–255).
    #[serde(default = "default_colour")]
    pub colour: (u8, u8, u8),
    #[serde(default = "default_tip")]
    pub tip_colour: (u8, u8, u8),
    /// How far from the camera it reaches, m (lower quality tiers draw less).
    #[serde(default = "default_draw")]
    pub draw_m: f32,
    /// How strongly it sways in the wind (0: still).
    #[serde(default = "one")]
    pub sway: f32,
    /// Bare ground kept beside roads, beyond the kerb or sidewalk, m.
    #[serde(default = "default_verge")]
    pub verge_m: f32,
    /// Steepest ground grass grows on (rise per metre).
    #[serde(default = "default_grade")]
    pub max_grade: f32,
    /// Where it grows on flat scenes: ((min x, min y), (max x, max y)), m.
    /// Absent: the terrain's square, else round the roads.
    #[serde(default)]
    pub area: Option<((f32, f32), (f32, f32))>,
}

fn default_density() -> f32 {
    6.0
}
fn default_height() -> f32 {
    0.45
}
fn default_colour() -> (u8, u8, u8) {
    (74, 104, 40)
}
fn default_tip() -> (u8, u8, u8) {
    (140, 168, 68)
}
fn default_draw() -> f32 {
    55.0
}
fn one() -> f32 {
    1.0
}
fn default_verge() -> f32 {
    0.3
}
fn default_grade() -> f32 {
    0.7
}

impl GrassDef {
    /// Problems with the values: (field, expected).
    pub fn problems(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if !(0.1..=40.0).contains(&self.density_per_m2) {
            out.push((
                "density_per_m2",
                format!("0.1 ≤ density_per_m2 ≤ 40, found {}", self.density_per_m2),
            ));
        }
        if !(0.02..=3.0).contains(&self.height_m) {
            out.push((
                "height_m",
                format!("0.02 ≤ height_m ≤ 3, found {}", self.height_m),
            ));
        }
        if !(5.0..=200.0).contains(&self.draw_m) {
            out.push(("draw_m", format!("5 ≤ draw_m ≤ 200, found {}", self.draw_m)));
        }
        for (field, v) in [
            ("sway", self.sway),
            ("verge_m", self.verge_m),
            ("max_grade", self.max_grade),
        ] {
            if !(v >= 0.0 && v.is_finite()) {
                out.push((field, format!("{field} ≥ 0, found {v}")));
            }
        }
        if let Some((a, b)) = self.area
            && !(b.0 > a.0 && b.1 > a.1)
        {
            out.push(("area", "max corner above and right of min".into()));
        }
        out
    }
}

/// Where grass may grow: 0–255 per cell of a grid `cell_m` apart.
#[derive(Clone, Debug, PartialEq)]
pub struct GrassMap {
    pub origin: Vec2,
    pub cell_m: f32,
    pub nx: usize,
    pub ny: usize,
    pub mask: Vec<u8>,
}

impl GrassMap {
    /// Mask at `p` (nearest cell), 0 outside.
    pub fn at(&self, p: Vec2) -> u8 {
        let q = (p - self.origin) / self.cell_m;
        if q.x < 0.0 || q.y < 0.0 {
            return 0;
        }
        let (i, j) = (q.x as usize, q.y as usize);
        if i >= self.nx || j >= self.ny {
            return 0;
        }
        self.mask[j * self.nx + i]
    }
}

/// A scene's grass: settings and mask.
#[derive(Clone, Debug)]
pub struct BuiltGrass {
    pub def: GrassDef,
    pub map: GrassMap,
}

/// The current scene's grass, for drawing.
#[derive(Clone, Debug)]
pub struct CurrentGrass(pub std::sync::Arc<BuiltGrass>);

/// Most cells in a mask; larger areas get coarser cells.
const MAX_CELLS: usize = 4_000_000;
/// Grass round the roads of a scene without terrain or an area, m.
const ROAD_MARGIN_M: f32 = 150.0;

/// Where `def`'s grass grows, given the scene's roads, buildings and land.
pub fn build(
    def: &GrassDef,
    net: Option<&RoadNetwork>,
    scenery: Option<&crate::scenery::Scenery>,
    land: Option<&HeightField>,
    water: Option<&wp_physics::WaterLevels>,
) -> GrassMap {
    // The area: as given, else the terrain's square, else round the roads.
    let (lo, hi) = match (def.area, land, net) {
        (Some((a, b)), _, _) => (Vec2::from(a), Vec2::from(b)),
        (None, Some(f), _) => (f.origin, f.max()),
        (None, None, Some(n)) => {
            let pts = n.roads.iter().flat_map(|r| r.centre.points.iter().copied());
            let (lo, hi) = pts.fold(
                (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
                |(l, h), p| (l.min(p), h.max(p)),
            );
            if lo.x > hi.x {
                (Vec2::ZERO, Vec2::ZERO)
            } else {
                (
                    lo - Vec2::splat(ROAD_MARGIN_M),
                    hi + Vec2::splat(ROAD_MARGIN_M),
                )
            }
        }
        _ => (Vec2::splat(-100.0), Vec2::splat(100.0)),
    };
    let size = (hi - lo).max(Vec2::ONE);
    let mut cell_m = 1.0f32;
    while (size.x / cell_m) as usize * (size.y / cell_m) as usize > MAX_CELLS {
        cell_m *= 1.5;
    }
    let nx = (size.x / cell_m).ceil() as usize + 1;
    let ny = (size.y / cell_m).ceil() as usize + 1;
    let mut map = GrassMap {
        origin: lo,
        cell_m,
        nx,
        ny,
        mask: vec![255; nx * ny],
    };
    let point = |i: usize, j: usize| lo + Vec2::new(i as f32 + 0.5, j as f32 + 0.5) * cell_m;
    // Steep ground: thinning from 70 % of the limit to none at it; none in
    // water or right at its edge.
    if let Some(f) = land {
        for j in 0..ny {
            for i in 0..nx {
                let g = f.land_gradient(point(i, j)).length();
                let p = point(i, j);
                // None under bridges.
                if f.decks.height(p).is_some() {
                    map.mask[j * nx + i] = 0;
                    continue;
                }
                let dry = water
                    .and_then(|w| w.level_at(p))
                    .map_or(1.0, |l| smooth(l + 0.15, l + 0.45, f.land_height(p)));
                let keep = (1.0 - smooth(def.max_grade * 0.7, def.max_grade.max(1e-3), g)) * dry;
                let m = &mut map.mask[j * nx + i];
                *m = (*m as f32 * keep) as u8;
            }
        }
    }
    // Roads, sidewalks and junctions: bare, with a soft 1 m edge.
    if let Some(net) = net {
        let soft = 0.7;
        let mut clear = |a: Vec2, b: Vec2, half: f32| {
            let reach = half + soft;
            let i0 = (((a.x.min(b.x) - reach - lo.x) / cell_m).floor().max(0.0)) as usize;
            let j0 = (((a.y.min(b.y) - reach - lo.y) / cell_m).floor().max(0.0)) as usize;
            let i1 = ((((a.x.max(b.x) + reach - lo.x) / cell_m).ceil()).max(0.0) as usize).min(nx);
            let j1 = ((((a.y.max(b.y) + reach - lo.y) / cell_m).ceil()).max(0.0) as usize).min(ny);
            let ab = b - a;
            let len2 = ab.length_squared().max(1e-6);
            for j in j0..j1 {
                for i in i0..i1 {
                    let p = point(i, j);
                    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                    let d = p.distance(a + ab * t) - half;
                    let keep = smooth(0.0, soft, d);
                    let m = &mut map.mask[j * nx + i];
                    *m = (*m as f32 * keep) as u8;
                }
            }
        };
        for r in &net.roads {
            let half = r.left_m.max(r.right_m) + r.def.sidewalk_m + def.verge_m;
            let pts = r.centre.resample(2.0).points;
            for w in pts.windows(2) {
                clear(w[0], w[1], half);
            }
        }
        for jn in &net.junctions {
            let r = jn
                .corners
                .iter()
                .map(|c| c.distance(jn.position))
                .fold(0.0f32, f32::max)
                + def.verge_m;
            clear(jn.position, jn.position, r.max(6.0));
        }
        // Lane connectors (bends at two-road nodes have no junction).
        for lane in &net.lanes {
            if lane.approach.is_none() {
                continue;
            }
            let pts = lane.path.resample(2.0).points;
            for w in pts.windows(2) {
                clear(w[0], w[1], lane.width_m / 2.0 + def.verge_m);
            }
        }
    }
    // Inside buildings.
    if let Some(s) = scenery {
        for b in &s.buildings {
            let (bl, bh) = b.footprint.iter().fold(
                (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
                |(l, h), p| (l.min(*p), h.max(*p)),
            );
            let i0 = (((bl.x - lo.x) / cell_m).floor().max(0.0)) as usize;
            let j0 = (((bl.y - lo.y) / cell_m).floor().max(0.0)) as usize;
            let i1 = ((((bh.x - lo.x) / cell_m).ceil()).max(0.0) as usize).min(nx);
            let j1 = ((((bh.y - lo.y) / cell_m).ceil()).max(0.0) as usize).min(ny);
            for j in j0..j1 {
                for i in i0..i1 {
                    if inside(&b.footprint, point(i, j)) {
                        map.mask[j * nx + i] = 0;
                    }
                }
            }
        }
    }
    map
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Point in polygon (even-odd).
fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_keeps_off_roads() {
        let def: GrassDef = wp_data::parse_ron("g.ron", "()").unwrap();
        assert!(def.problems().is_empty());
        let net: wp_road::RoadNetworkDef = wp_data::parse_ron(
            "r.ron",
            "(name: \"r\", nodes: [(id: \"a\", position: (-100, 0)), (id: \"b\", position: (100, 0))],
              roads: [(id: \"ab\", from: \"a\", to: \"b\", sidewalk_m: 2.0)])",
        )
        .unwrap();
        let net = RoadNetwork::build(&net);
        let map = build(&def, Some(&net), None, None, None);
        // On the road and its sidewalk: bare. Well away: full.
        assert_eq!(map.at(Vec2::new(0.0, 0.5)), 0);
        assert_eq!(map.at(Vec2::new(0.0, -5.0)), 0);
        assert_eq!(map.at(Vec2::new(0.0, 20.0)), 255);
        assert_eq!(map.at(Vec2::new(-200.0, 60.0)), 255);
        // Outside the area: none.
        assert_eq!(map.at(Vec2::new(0.0, 400.0)), 0);
        assert!(inside(
            &[Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
            Vec2::splat(0.5)
        ));
    }
}
