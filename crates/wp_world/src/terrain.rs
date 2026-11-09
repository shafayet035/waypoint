//! Terrain: hills and valleys from a scene's `terrain` settings, shaped so
//! roads run on it. The land's own shape (rolling noise plus hand-placed
//! hills) is sampled along every road and smoothed into the road's height
//! profile; the land is then levelled across each road and its shoulders and
//! blends back to its own shape further out. Junctions sit level at their
//! node's height. The result is one [`HeightField`] that physics drives on
//! and the renderer draws ([`build_tiles`]).

use glam::{Vec2, Vec3};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wp_physics::HeightField;
use wp_road::RoadNetwork;

/// A scene's land shape around the flat plain (height 0). Hills with a
/// negative height dig hollows, which water fills up to its level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainDef {
    /// Centre of the terrain square (x east, y north), m.
    #[serde(default)]
    pub centre: (f32, f32),
    /// Width east–west and north–south, m.
    pub size_m: (f32, f32),
    /// Grid spacing, m (2–20). Smaller follows roads more closely and costs more.
    #[serde(default = "default_cell")]
    pub cell_m: f32,
    /// Rolling land from layered noise.
    #[serde(default)]
    pub noise: Option<NoiseDef>,
    /// Level areas (an oasis, a town, a car park), before the hills: a
    /// hollow placed on a flat holds a pond.
    #[serde(default)]
    pub flats: Vec<FlatDef>,
    /// Hand-placed hills (a negative height digs a hollow).
    #[serde(default)]
    pub hills: Vec<HillDef>,
    /// Level ground either side of a road, beyond its kerb or sidewalk, m.
    #[serde(default = "default_shoulder")]
    pub shoulder_m: f32,
    /// Distance over which the land rises or falls from a road's height back
    /// to its own, m. Shorter gives cuttings and embankments.
    #[serde(default = "default_blend")]
    pub blend_m: f32,
    /// Roads follow the land smoothed over this length, m: longer gives
    /// gentler gradients and deeper cuttings.
    #[serde(default = "default_smoothing")]
    pub road_smoothing_m: f32,
    /// The land flattens to 0 over this distance inside the terrain's edge, m.
    #[serde(default = "default_edge")]
    pub edge_m: f32,
    /// Low-poly facets (one flat face per triangle) instead of smooth shading.
    #[serde(default)]
    pub faceted: bool,
    /// Colour of the land (sRGB 0–255); absent: the look's ground colour.
    #[serde(default)]
    pub colour: Option<(u8, u8, u8)>,
    /// Colour of steep ground (sRGB 0–255).
    #[serde(default = "default_rock")]
    pub rock: (u8, u8, u8),
    /// Slope at which ground turns fully to rock colour (rise per metre).
    #[serde(default = "default_rock_from")]
    pub rock_from_grade: f32,
}

/// Rolling land: `octaves` layers of smooth noise, each half the size and
/// half the height of the last.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoiseDef {
    /// Height of the biggest swells, m.
    pub height_m: f32,
    /// Distance between the biggest swells, m.
    pub wavelength_m: f32,
    #[serde(default = "default_octaves")]
    pub octaves: u8,
    #[serde(default)]
    pub seed: u32,
    /// Sharp ridges instead of round swells.
    #[serde(default)]
    pub ridged: bool,
}

/// A round hill with a smooth top and foot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HillDef {
    /// Top of the hill (x, y), m.
    pub position: (f32, f32),
    /// Distance from the top to the foot, m.
    pub radius_m: f32,
    /// Height of the top, m (negative: a hollow).
    pub height_m: f32,
}

/// A level, round area that blends back into the land around it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FlatDef {
    /// Centre (x, y), m.
    pub position: (f32, f32),
    /// Radius of the level part, m.
    pub radius_m: f32,
    /// Its height, m (absent: the land's height at the centre).
    #[serde(default)]
    pub height_m: Option<f32>,
    /// Distance over which it blends into the land around, m.
    #[serde(default = "default_flat_blend")]
    pub blend_m: f32,
}

fn default_flat_blend() -> f32 {
    30.0
}

fn default_cell() -> f32 {
    4.0
}
fn default_shoulder() -> f32 {
    3.0
}
fn default_blend() -> f32 {
    35.0
}
fn default_smoothing() -> f32 {
    150.0
}
fn default_edge() -> f32 {
    120.0
}
fn default_rock() -> (u8, u8, u8) {
    (128, 112, 100)
}
fn default_rock_from() -> f32 {
    0.9
}
fn default_octaves() -> u8 {
    4
}

/// Most grid points a terrain may have (about 2 km × 2 km at 2 m).
pub const MAX_POINTS: usize = 1_100_000;

impl TerrainDef {
    /// Problems with the values: (field, message).
    pub fn problems(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if !(2.0..=20.0).contains(&self.cell_m) {
            out.push(("cell_m", format!("2 ≤ cell_m ≤ 20, found {}", self.cell_m)));
        }
        if !(self.size_m.0 >= 50.0 && self.size_m.1 >= 50.0) {
            out.push((
                "size_m",
                format!("both sides at least 50 m, found {:?}", self.size_m),
            ));
        }
        for (field, v) in [
            ("shoulder_m", self.shoulder_m),
            ("blend_m", self.blend_m),
            ("road_smoothing_m", self.road_smoothing_m),
            ("edge_m", self.edge_m),
        ] {
            if !(v >= 0.0 && v.is_finite()) {
                out.push((field, format!("{field} ≥ 0, found {v}")));
            }
        }
        if let Some(n) = &self.noise
            && !(n.wavelength_m >= 10.0 && n.height_m.is_finite() && (1..=8).contains(&n.octaves))
        {
            out.push((
                "noise",
                format!(
                    "wavelength_m ≥ 10 and 1 ≤ octaves ≤ 8, found {} and {}",
                    n.wavelength_m, n.octaves
                ),
            ));
        }
        for h in &self.hills {
            if !(h.radius_m > 0.0 && h.height_m.is_finite()) {
                out.push(("hills", format!("radius_m > 0, found {}", h.radius_m)));
            }
        }
        for f in &self.flats {
            if !(f.radius_m > 0.0 && f.blend_m >= 0.0) {
                out.push((
                    "flats",
                    format!(
                        "radius_m > 0 and blend_m ≥ 0, found {} and {}",
                        f.radius_m, f.blend_m
                    ),
                ));
            }
        }
        out
    }

    /// Grid size (points along x, along y).
    pub fn grid(&self) -> (usize, usize) {
        let n = |len: f32| (len / self.cell_m.max(0.5)).ceil().max(1.0) as usize + 1;
        (n(self.size_m.0), n(self.size_m.1))
    }

    /// The land's own height at `p`, before roads level it.
    pub fn natural(&self, p: Vec2) -> f32 {
        let mut h = 0.0;
        if let Some(n) = &self.noise {
            h += fbm(n, p);
        }
        for flat in &self.flats {
            let c = Vec2::from(flat.position);
            let level = flat
                .height_m
                .unwrap_or_else(|| self.noise.as_ref().map_or(0.0, |n| fbm(n, c)));
            let d = p.distance(c) - flat.radius_m;
            let w = 1.0 - smoothstep(0.0, flat.blend_m.max(0.01), d);
            h += (level - h) * w;
        }
        for hill in &self.hills {
            let d = p.distance(Vec2::from(hill.position)) / hill.radius_m;
            if d < 1.0 {
                // Smooth at the top and at the foot.
                let t = 1.0 - d * d;
                h += hill.height_m * t * t * (3.0 - 2.0 * t);
            }
        }
        // Flatten towards the edge.
        let lo = Vec2::from(self.centre) - Vec2::from(self.size_m) * 0.5;
        let hi = lo + Vec2::from(self.size_m);
        let inside = (p - lo).min(hi - p).min_element();
        let fade = if self.edge_m > 0.0 {
            smoothstep(0.0, self.edge_m, inside)
        } else {
            1.0
        };
        h * fade
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A repeatable hash of a lattice point to 0–1.
fn lattice(seed: u32, i: i32, j: i32) -> f32 {
    let mut x = (i as u32).wrapping_mul(0x8DA6_B343)
        ^ (j as u32).wrapping_mul(0xD816_3841)
        ^ seed.wrapping_mul(0xCB1A_B31F);
    x ^= x >> 13;
    x = x.wrapping_mul(0x5BD1_E995);
    x ^= x >> 15;
    (x & 0x00FF_FFFF) as f32 / 16_777_215.0
}

/// Smooth value noise, 0–1.
fn value_noise(seed: u32, p: Vec2) -> f32 {
    let (fi, fj) = (p.x.floor(), p.y.floor());
    let (i, j) = (fi as i32, fj as i32);
    let (u, v) = (p.x - fi, p.y - fj);
    let (u, v) = (u * u * (3.0 - 2.0 * u), v * v * (3.0 - 2.0 * v));
    let a = lattice(seed, i, j);
    let b = lattice(seed, i + 1, j);
    let c = lattice(seed, i, j + 1);
    let d = lattice(seed, i + 1, j + 1);
    let low = a + (b - a) * u;
    let high = c + (d - c) * u;
    low + (high - low) * v
}

/// Layered noise scaled so the land spans 0 to about `height_m`.
fn fbm(n: &NoiseDef, p: Vec2) -> f32 {
    let mut sum = 0.0;
    let mut norm = 0.0;
    let mut amp = 1.0;
    let mut q = p / n.wavelength_m;
    for k in 0..n.octaves.max(1) as u32 {
        let v = value_noise(n.seed.wrapping_add(k * 7919), q);
        let v = if n.ridged {
            1.0 - (2.0 * v - 1.0).abs()
        } else {
            v
        };
        sum += v * amp;
        norm += amp;
        amp *= 0.5;
        // Turn each octave a little so the lattice does not line up.
        q = Vec2::new(q.x * 1.6 - q.y * 1.2, q.x * 1.2 + q.y * 1.6);
    }
    // Stretch the middle of the range: most land sits near the middle.
    let v = sum / norm;
    n.height_m * smoothstep(0.15, 0.85, v)
}

/// A road's height profile: heights every `step` metres along its centreline.
struct Profile {
    step: f32,
    heights: Vec<f32>,
}

impl Profile {
    fn at(&self, s: f32) -> f32 {
        let q = (s / self.step).max(0.0);
        let i = (q.floor() as usize).min(self.heights.len().saturating_sub(2));
        let t = (q - i as f32).clamp(0.0, 1.0);
        let a = self.heights[i];
        let b = self.heights[(i + 1).min(self.heights.len() - 1)];
        a + (b - a) * t
    }
}

/// Spacing of road profile samples, m.
const PROFILE_STEP_M: f32 = 4.0;

/// The land's height averaged over a disc (for junctions).
fn disc_average(def: &TerrainDef, c: Vec2, r: f32) -> f32 {
    let mut sum = def.natural(c);
    let mut n = 1.0;
    for ring in 1..=2 {
        let rr = r * ring as f32 / 2.0;
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::TAU / 8.0;
            sum += def.natural(c + Vec2::from_angle(a) * rr);
            n += 1.0;
        }
    }
    sum / n
}

/// Shape the terrain: the land, levelled under `net`'s roads.
pub fn build(def: &TerrainDef, net: Option<&RoadNetwork>, water: &[WaterDef]) -> HeightField {
    // Roads stay out of water that covers all low land (no bridges yet).
    let road_floor = water
        .iter()
        .filter(|w| w.at.is_none())
        .map(|w| w.level_m + ROAD_ABOVE_WATER_M)
        .fold(f32::MIN, f32::max);
    let (nx, ny) = def.grid();
    let origin = Vec2::from(def.centre) - Vec2::from(def.size_m) * 0.5;
    let mut field = HeightField::flat(origin, def.cell_m, nx, ny);
    for j in 0..ny {
        for i in 0..nx {
            field.heights[j * nx + i] = def.natural(field.point(i, j));
        }
    }
    let Some(net) = net else {
        return field;
    };
    // Where each grid point is pulled by a road: (distance beyond the
    // level band, road height there). The nearest band wins.
    let mut pull: Vec<(f32, f32)> = vec![(f32::MAX, 0.0); nx * ny];
    let reach = def.shoulder_m + def.blend_m;
    let mut mark = |field: &HeightField, a: Vec2, b: Vec2, ha: f32, hb: f32, half: f32| {
        let lo = a.min(b) - Vec2::splat(half + reach);
        let hi = a.max(b) + Vec2::splat(half + reach);
        let i0 = ((lo.x - origin.x) / def.cell_m).floor().max(0.0) as usize;
        let j0 = ((lo.y - origin.y) / def.cell_m).floor().max(0.0) as usize;
        let i1 = (((hi.x - origin.x) / def.cell_m).ceil().max(0.0) as usize).min(nx - 1);
        let j1 = (((hi.y - origin.y) / def.cell_m).ceil().max(0.0) as usize).min(ny - 1);
        let ab = b - a;
        let len2 = ab.length_squared().max(1e-6);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let p = field.point(i, j);
                let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                let d = (p.distance(a + ab * t) - half).max(0.0);
                let k = j * nx + i;
                if d < pull[k].0 {
                    pull[k] = (d, ha + (hb - ha) * t);
                }
            }
        }
    };

    // Node heights: the land averaged round each node (shared by every road
    // that meets there, so they join level).
    let node_count = net
        .roads
        .iter()
        .map(|r| r.from_node.max(r.to_node) + 1)
        .max()
        .unwrap_or(0);
    let mut node_pos = vec![(Vec2::ZERO, 0.0f32); node_count];
    let mut node_half = vec![0.0f32; node_count];
    for r in &net.roads {
        if r.centre.points.len() < 2 {
            continue;
        }
        let half = r.left_m.max(r.right_m) + r.def.sidewalk_m;
        for (node, p) in [
            (r.from_node, r.centre.points[0]),
            (r.to_node, *r.centre.points.last().expect("two points")),
        ] {
            node_pos[node].0 += p;
            node_pos[node].1 += 1.0;
            node_half[node] = node_half[node].max(half);
        }
    }
    let nodes: Vec<Option<(Vec2, f32)>> = node_pos
        .iter()
        .zip(&node_half)
        .map(|((sum, n), half)| {
            (*n > 0.0).then(|| {
                let c = *sum / *n;
                (c, disc_average(def, c, (half * 2.0).max(20.0)))
            })
        })
        .collect();

    for r in &net.roads {
        let c = &r.centre;
        let len = c.length();
        if len < 0.5 {
            continue;
        }
        let half = r.left_m.max(r.right_m) + r.def.sidewalk_m + def.shoulder_m;
        // The land along the road, smoothed with a moving average.
        let n = (len / PROFILE_STEP_M).ceil() as usize + 1;
        let step = len / (n - 1) as f32;
        let raw: Vec<f32> = (0..n)
            .map(|k| def.natural(c.sample(k as f32 * step).0))
            .collect();
        let w = (def.road_smoothing_m / step / 2.0).round() as usize;
        let mut prefix = vec![0.0f64; n + 1];
        for k in 0..n {
            prefix[k + 1] = prefix[k] + raw[k] as f64;
        }
        let mut heights: Vec<f32> = (0..n)
            .map(|k| {
                let a = k.saturating_sub(w);
                let b = (k + w).min(n - 1);
                ((prefix[b + 1] - prefix[a]) / (b + 1 - a) as f64) as f32
            })
            .collect();
        // Pin both ends to their nodes' heights.
        let start = nodes[r.from_node].map_or(heights[0], |n| n.1);
        let end = nodes[r.to_node].map_or(heights[n - 1], |n| n.1);
        let (d0, d1) = (start - heights[0], end - heights[n - 1]);
        for (k, h) in heights.iter_mut().enumerate() {
            let t = k as f32 / (n - 1) as f32;
            *h = (*h + d0 + (d1 - d0) * t).max(road_floor);
        }
        let profile = Profile { step, heights };
        // Level the band along the road.
        let samples = c.resample(def.cell_m.min(PROFILE_STEP_M));
        let mut s = 0.0;
        for k in 1..samples.points.len() {
            let (a, b) = (samples.points[k - 1], samples.points[k]);
            let seg = a.distance(b);
            mark(&field, a, b, profile.at(s), profile.at(s + seg), half);
            s += seg;
        }
    }
    // Junction areas sit level at the node height.
    for (node, info) in nodes.iter().enumerate() {
        if let Some((c, h)) = info {
            let r = node_half[node] + def.shoulder_m + 4.0;
            mark(&field, *c, *c, *h, *h, r);
        }
    }

    for (k, (d, road_h)) in pull.iter().enumerate() {
        if *d == f32::MAX {
            continue;
        }
        let w = 1.0 - smoothstep(0.0, def.blend_m.max(0.01), *d);
        let h = &mut field.heights[k];
        *h += (road_h - *h) * w;
    }
    field
}

/// How far roads stay above water, m.
pub const ROAD_ABOVE_WATER_M: f32 = 0.6;

/// A body of standing water: a lake, pond or ford.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaterDef {
    /// Height of the surface, m. The plain is at 0, so low water sits in
    /// hollows (terrain hills with negative height_m).
    #[serde(default = "default_level")]
    pub level_m: f32,
    /// A point in the lake (x, y), m: only the hollow round it fills, so
    /// lakes can sit up in the hills. Absent: all land below the level,
    /// and roads are kept above it.
    #[serde(default)]
    pub at: Option<(f32, f32)>,
    /// Colour of shallow water and of deep water (sRGB 0–255).
    #[serde(default = "default_shallow")]
    pub colour: (u8, u8, u8),
    #[serde(default = "default_deep")]
    pub deep_colour: (u8, u8, u8),
    /// Depth at which the bottom can no longer be seen, m.
    #[serde(default = "default_clarity")]
    pub clarity_m: f32,
    /// Ripple strength (0: a mirror).
    #[serde(default = "default_waves")]
    pub waves: f32,
}

fn default_level() -> f32 {
    -0.5
}
fn default_shallow() -> (u8, u8, u8) {
    (72, 168, 170)
}
fn default_deep() -> (u8, u8, u8) {
    (20, 64, 96)
}
fn default_clarity() -> f32 {
    3.0
}
fn default_waves() -> f32 {
    1.0
}

impl WaterDef {
    pub fn problems(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if !self.level_m.is_finite() {
            out.push(("level_m", "a finite height".into()));
        }
        if !(self.clarity_m > 0.0) {
            out.push((
                "clarity_m",
                format!("clarity_m > 0, found {}", self.clarity_m),
            ));
        }
        if !(self.waves >= 0.0 && self.waves <= 5.0) {
            out.push(("waves", format!("0 ≤ waves ≤ 5, found {}", self.waves)));
        }
        out
    }
}

/// No water over a grid point.
pub const DRY: u8 = u8::MAX;

/// Fill the water bodies on `field`: for each grid point, the body over it
/// (index into `bodies`, or [`DRY`]), and the surface heights. A body with
/// `at` fills the hollow round that point (land lower than its level, joined
/// to it); one without fills all low land. Earlier bodies win.
pub fn flood(field: &HeightField, bodies: &[WaterDef]) -> (Vec<u8>, wp_physics::WaterLevels) {
    let (nx, ny) = (field.nx, field.ny);
    let mut owner = vec![DRY; nx * ny];
    for (k, body) in bodies.iter().enumerate().take(DRY as usize) {
        let below = |i: usize, j: usize| field.at(i, j) < body.level_m;
        match body.at {
            None => {
                for j in 0..ny {
                    for i in 0..nx {
                        if owner[j * nx + i] == DRY && below(i, j) {
                            owner[j * nx + i] = k as u8;
                        }
                    }
                }
            }
            Some(at) => {
                let q = ((Vec2::from(at) - field.origin) / field.cell_m).round();
                if q.x < 0.0 || q.y < 0.0 || q.x as usize >= nx || q.y as usize >= ny {
                    continue;
                }
                let start = (q.x as usize, q.y as usize);
                if !below(start.0, start.1) {
                    continue;
                }
                let mut stack = vec![start];
                while let Some((i, j)) = stack.pop() {
                    let idx = j * nx + i;
                    if owner[idx] != DRY || !below(i, j) {
                        continue;
                    }
                    owner[idx] = k as u8;
                    if i > 0 {
                        stack.push((i - 1, j));
                    }
                    if i + 1 < nx {
                        stack.push((i + 1, j));
                    }
                    if j > 0 {
                        stack.push((i, j - 1));
                    }
                    if j + 1 < ny {
                        stack.push((i, j + 1));
                    }
                }
            }
        }
    }
    let levels = owner
        .iter()
        .map(|&o| {
            if o == DRY {
                f32::NAN
            } else {
                bodies[o as usize].level_m
            }
        })
        .collect();
    let map = wp_physics::WaterLevels {
        origin: field.origin,
        cell_m: field.cell_m,
        nx,
        ny,
        levels,
    };
    (owner, map)
}

/// Body `body`'s surface (at height `level`) as meshes in tiles (the land's
/// tiling), over cells with a corner under it. Positions are relative to
/// each tile's centre; `colors[k][0]` holds the water depth at each vertex
/// (0 where the land is above it, at the shore).
pub fn build_water_tiles(
    field: &HeightField,
    owner: &[u8],
    body: u8,
    level: f32,
) -> Vec<TerrainTile> {
    let (nx, ny) = (field.nx, field.ny);
    let mut tiles = Vec::new();
    let mut tj = 0;
    while tj < ny - 1 {
        let mut ti = 0;
        while ti < nx - 1 {
            let (i1, j1) = ((ti + TILE_CELLS).min(nx - 1), (tj + TILE_CELLS).min(ny - 1));
            let centre = (field.point(ti, tj) + field.point(i1, j1)) * 0.5;
            let mut t = TerrainTile {
                centre: centre.to_array(),
                min: [f32::MAX; 3],
                max: [f32::MIN; 3],
                ..Default::default()
            };
            for j in tj..j1 {
                for i in ti..i1 {
                    let q = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                    if !q.iter().any(|&(i, j)| owner[j * nx + i] == body) {
                        continue;
                    }
                    let base = t.positions.len() as u32;
                    for (i, j) in q {
                        let p = field.point(i, j) - centre;
                        t.positions.push([p.x, p.y, level]);
                        t.normals.push([0.0, 0.0, 1.0]);
                        t.colors
                            .push([(level - field.at(i, j)).max(0.0), 0.0, 0.0, 1.0]);
                    }
                    t.indices.extend_from_slice(&[
                        base,
                        base + 1,
                        base + 2,
                        base,
                        base + 2,
                        base + 3,
                    ]);
                }
            }
            if !t.indices.is_empty() {
                for p in &t.positions {
                    t.min = std::array::from_fn(|a| t.min[a].min(p[a]));
                    t.max = std::array::from_fn(|a| t.max[a].max(p[a]));
                }
                tiles.push(t);
            }
            ti = i1;
        }
        tj = (tj + TILE_CELLS).min(ny - 1);
    }
    tiles
}

/// The steepest stretch of road on `field`: (rise per metre, where).
pub fn steepest_road(field: &HeightField, net: &RoadNetwork) -> Option<(f32, Vec2)> {
    let mut best: Option<(f32, Vec2)> = None;
    for r in &net.roads {
        let pts = r.centre.resample(10.0).points;
        for w in pts.windows(2) {
            let run = w[0].distance(w[1]);
            if run < 1.0 {
                continue;
            }
            let g = (field.height(w[1]) - field.height(w[0])).abs() / run;
            if best.is_none_or(|b| g > b.0) {
                best = Some((g, (w[0] + w[1]) * 0.5));
            }
        }
    }
    best
}

/// A scene's terrain, shaped: its settings and the heights, and its water
/// (the bodies, and which body is over each grid point: see [`flood`]).
#[derive(Clone, Debug)]
pub struct BuiltTerrain {
    pub def: TerrainDef,
    pub field: std::sync::Arc<HeightField>,
    pub water: Vec<WaterDef>,
    pub water_owner: Vec<u8>,
    pub water_levels: Option<std::sync::Arc<wp_physics::WaterLevels>>,
}

/// The current scene's terrain, for drawing (physics reads
/// [`wp_physics::Terrain`], which shares the heights).
#[derive(Clone, Debug)]
pub struct CurrentTerrain(pub std::sync::Arc<BuiltTerrain>);

/// A square piece of the land mesh, positions relative to `centre`.
#[derive(Clone, Debug, Default)]
pub struct TerrainTile {
    pub centre: [f32; 2],
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex colour (linear RGB, alpha 1).
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Cells per tile side: tiles are about 400 m at 4 m cells.
const TILE_CELLS: usize = 100;

/// How far the land mesh sits below the heights, m: roads and everything
/// on them are drawn on the true heights, so the land never shows through
/// them where its flat triangles bulge over the bilinear surface.
pub const MESH_DROP_M: f32 = 0.06;

/// The land as meshes in tiles, coloured from its own colour (else `ground`,
/// the look's ground colour, linear) to `rock` on steep slopes.
pub fn build_tiles(def: &TerrainDef, field: &HeightField, ground: Vec3) -> Vec<TerrainTile> {
    let rock = Vec3::from(crate::srgb([def.rock.0, def.rock.1, def.rock.2]));
    let ground = def
        .colour
        .map_or(ground, |c| Vec3::from(crate::srgb([c.0, c.1, c.2])));
    let (nx, ny) = (field.nx, field.ny);
    let mut tiles = Vec::new();
    let normal_at = |i: usize, j: usize| {
        let g = field.gradient(field.point(i, j) + Vec2::splat(0.01));
        Vec3::new(-g.x, -g.y, 1.0).normalize()
    };
    let tint = |n: Vec3, p: Vec2| -> [f32; 4] {
        let grade = (1.0 - n.z * n.z).max(0.0).sqrt() / n.z.max(0.05);
        let r = smoothstep(def.rock_from_grade * 0.45, def.rock_from_grade, grade);
        // A little mottling so large flats are not one flat colour.
        let m = 0.92 + 0.16 * value_noise(77, p / 23.0);
        let c = ground.lerp(rock, r) * m;
        [c.x, c.y, c.z, 1.0]
    };
    let mut tj = 0;
    while tj < ny - 1 {
        let mut ti = 0;
        while ti < nx - 1 {
            let (i1, j1) = ((ti + TILE_CELLS).min(nx - 1), (tj + TILE_CELLS).min(ny - 1));
            let a = field.point(ti, tj);
            let b = field.point(i1, j1);
            let centre = (a + b) * 0.5;
            let mut t = TerrainTile {
                centre: centre.to_array(),
                min: [f32::MAX; 3],
                max: [f32::MIN; 3],
                ..Default::default()
            };
            let local = |i: usize, j: usize| {
                let p = field.point(i, j) - centre;
                [p.x, p.y, field.at(i, j) - MESH_DROP_M]
            };
            if def.faceted {
                // Three vertices per triangle, each with the face's normal.
                for j in tj..j1 {
                    for i in ti..i1 {
                        let q = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                        // Alternate the diagonal for a less regular look.
                        let tris = if (i + j) % 2 == 0 {
                            [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]
                        } else {
                            [[q[0], q[1], q[3]], [q[1], q[2], q[3]]]
                        };
                        for tri in tris {
                            let p: Vec<Vec3> =
                                tri.iter().map(|&(i, j)| Vec3::from(local(i, j))).collect();
                            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Z);
                            let mid = (p[0] + p[1] + p[2]) / 3.0;
                            let c = tint(n, mid.truncate() + centre);
                            for v in &p {
                                t.indices.push(t.positions.len() as u32);
                                t.positions.push(v.to_array());
                                t.normals.push(n.to_array());
                                t.colors.push(c);
                            }
                        }
                    }
                }
            } else {
                let w = i1 - ti + 1;
                for j in tj..=j1 {
                    for i in ti..=i1 {
                        let n = normal_at(i, j);
                        t.positions.push(local(i, j));
                        t.normals.push(n.to_array());
                        t.colors.push(tint(n, field.point(i, j)));
                    }
                }
                for j in 0..(j1 - tj) {
                    for i in 0..(i1 - ti) {
                        let k = (j * w + i) as u32;
                        let w = w as u32;
                        t.indices
                            .extend_from_slice(&[k, k + 1, k + w + 1, k, k + w + 1, k + w]);
                    }
                }
            }
            for p in &t.positions {
                t.min = std::array::from_fn(|a| t.min[a].min(p[a]));
                t.max = std::array::from_fn(|a| t.max[a].max(p[a]));
            }
            tiles.push(t);
            ti = i1;
        }
        tj = (tj + TILE_CELLS).min(ny - 1);
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> TerrainDef {
        wp_data::parse_ron(
            "t.ron",
            "(size_m: (600, 600), noise: Some((height_m: 40, wavelength_m: 200)),
              hills: [(position: (150, 150), radius_m: 120, height_m: 30)])",
        )
        .unwrap()
    }

    fn road() -> RoadNetwork {
        let net: wp_road::RoadNetworkDef = wp_data::parse_ron(
            "r.ron",
            "(name: \"r\", nodes: [(id: \"a\", position: (-250, 0)), (id: \"b\", position: (250, 20))],
              roads: [(id: \"ab\", from: \"a\", to: \"b\", shape: Curve(through: [(0, 60)]))])",
        )
        .unwrap();
        RoadNetwork::build(&net)
    }

    #[test]
    fn land_is_hilly_and_flat_at_the_edge() {
        let d = def();
        assert!(d.problems().is_empty());
        let f = build(&d, None, &[]);
        let (lo, hi) = f.range();
        assert!(lo >= 0.0 && hi > 25.0, "{lo} {hi}");
        // Edge rows are flat at 0.
        assert_eq!(f.at(0, 40), 0.0);
        assert_eq!(f.at(f.nx - 1, 40), 0.0);
        // The same settings give the same land.
        assert_eq!(f, build(&d, None, &[]));
    }

    #[test]
    fn roads_are_level_across_and_gentle_along() {
        let d = def();
        let net = road();
        let f = build(&d, Some(&net), &[]);
        let r = &net.roads[0];
        let len = r.centre.length();
        let mut s = 10.0;
        while s < len - 10.0 {
            let (p, dir) = r.centre.sample(s);
            let left = p + dir.perp() * (r.left_m + 1.0);
            let right = p - dir.perp() * (r.right_m + 1.0);
            // Level across (within what bilinear cells can show).
            assert!(
                (f.height(left) - f.height(right)).abs() < 0.35,
                "at {s}: {} vs {}",
                f.height(left),
                f.height(right)
            );
            s += 7.0;
        }
        let (grade, _) = steepest_road(&f, &net).unwrap();
        assert!(grade < 0.12, "steepest {grade}");
        // Somewhere the road is off the ground plane (the land is hilly).
        let top = (0..20)
            .map(|k| f.height(r.centre.sample(len * k as f32 / 20.0).0))
            .fold(0.0f32, f32::max);
        assert!(top > 3.0, "road stays flat: {top}");
    }

    #[test]
    fn tiles_cover_the_field() {
        let mut d = def();
        let f = build(&d, None, &[]);
        let smooth = build_tiles(&d, &f, Vec3::splat(0.3));
        d.faceted = true;
        let faceted = build_tiles(&d, &f, Vec3::splat(0.3));
        let tris = |t: &[TerrainTile]| t.iter().map(|t| t.indices.len() / 3).sum::<usize>();
        let cells = (f.nx - 1) * (f.ny - 1);
        assert_eq!(tris(&smooth), cells * 2);
        assert_eq!(tris(&faceted), cells * 2);
        assert_eq!(smooth.len(), 4);
    }
}
