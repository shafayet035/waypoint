//! Scenery: buildings beside the roads.
//!
//! A scene names a scenery file (`scenery/<name>.scenery.ron`) holding
//! buildings placed by hand (an outline and a height) and `streets`: rules
//! that line roads with generated buildings. Buildings take a named style
//! (wall colours, roof, storey height, window spacing, shop fronts); the
//! renderer draws windows on the walls from it, lit at night.
//!
//! Everything here is pure: [`Scenery::build`] turns a definition (and the
//! road network, for the generated rows) into a list of [`Building`]s, and
//! [`build_tiles`] turns those into meshes cut into tiles for streaming.

use glam::{Vec2, Vec3};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wp_core::{Diagnostic, SimRng};
use wp_data::Loaded;
use wp_road::RoadNetwork;

/// `scenery/<name>.scenery.ron`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneryDef {
    pub name: String,
    /// Building styles by name. Built in: plaster, brick, concrete, office,
    /// industrial, shophouse; a style here with the same name replaces the built-in one.
    #[serde(default)]
    pub styles: BTreeMap<String, BuildingStyle>,
    /// Buildings placed one by one (imported maps list theirs here).
    #[serde(default)]
    pub buildings: Vec<BuildingDef>,
    /// Rows of generated buildings along roads.
    #[serde(default)]
    pub streets: Vec<StreetDef>,
    /// Trees placed one by one (imported maps list theirs here).
    #[serde(default)]
    pub trees: Vec<TreeDef>,
    /// Rows of street trees along kerbs (or verges where there is no sidewalk).
    #[serde(default)]
    pub tree_rows: Vec<TreeRowDef>,
    /// Models placed one by one (benches, bins, kiosks, hydrants...).
    #[serde(default)]
    pub props: Vec<PropDef>,
    /// Rows of models along kerbs (bins, hydrants, bollards, benches).
    #[serde(default)]
    pub prop_rows: Vec<PropRowDef>,
    /// Models for the street furniture the engine places itself (street
    /// lamps, bus stops) and for pedestrians; absent: built-in shapes.
    #[serde(default)]
    pub furniture: FurnitureDef,
}

/// A model placed by hand. Models live under `assets/` with a licence file
/// (`wp docs assets`); their origin is on the ground.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PropDef {
    /// Project path of the model, e.g. "assets/props/bench.glb".
    pub model: String,
    /// (x east, y north), m.
    pub position: (f32, f32),
    /// Which way the model's +X points, degrees anticlockwise from east.
    #[serde(default)]
    pub heading_deg: f32,
    /// Size relative to the model, 1: as modelled.
    #[serde(default = "one")]
    pub scale: f32,
    /// Half-width of a solid square round it, m (vehicles stop against it);
    /// 0: not solid (small clutter).
    #[serde(default)]
    pub solid_m: f32,
}

fn one() -> f32 {
    1.0
}

/// Props along roads, one every `spacing_m`, on the sidewalk `kerb_m` in from
/// the kerb with their +X facing the road; each spot picks one of `models`.
/// They skip junctions, crossings, street lamps, bus stops, trees, buildings
/// and each other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PropRowDef {
    /// Project paths of the models to pick from.
    pub models: Vec<String>,
    /// Road ids; empty: every road with a sidewalk.
    #[serde(default)]
    pub roads: Vec<String>,
    #[serde(default)]
    pub sides: Sides,
    /// Distance between props, m.
    #[serde(default = "default_prop_spacing")]
    pub spacing_m: f32,
    /// Distance in from the kerb, m.
    #[serde(default = "default_prop_kerb")]
    pub kerb_m: f32,
    /// Half-width of a solid square round each, m; 0: not solid.
    #[serde(default)]
    pub solid_m: f32,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_prop_spacing() -> f32 {
    30.0
}
fn default_prop_kerb() -> f32 {
    0.5
}

/// Models for the engine's own street furniture and people.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FurnitureDef {
    /// Street lamp: origin at the foot of the pole, the arm along +X (over
    /// the road). Materials named `head…` glow when the lamps are lit.
    #[serde(default)]
    pub street_lamp: Option<StreetLampModel>,
    /// Bus stop: origin at the stop's point on the kerb, +Y away from the
    /// road (into the sidewalk), +X along the kerb.
    #[serde(default)]
    pub bus_stop: Option<String>,
    /// People: standing figures with their feet at the origin, facing +X;
    /// each pedestrian picks one.
    #[serde(default)]
    pub people: Vec<String>,
}

/// A street lamp model and where its light comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StreetLampModel {
    /// Project path of the model.
    pub model: String,
    /// The lamp head: (along the arm, height), m.
    #[serde(default = "default_lamp_head")]
    pub head_m: (f32, f32),
}

fn default_lamp_head() -> (f32, f32) {
    (1.45, 7.2)
}

/// The shape of a tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TreeKind {
    /// Broadleaf: a round crown on a short trunk.
    #[default]
    Round,
    /// Conifer: a tall cone.
    Cone,
    /// Palm: a tall bare trunk with a flat crown.
    Palm,
}

/// A tree placed by hand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TreeDef {
    /// (x east, y north), m.
    pub position: (f32, f32),
    #[serde(default = "default_tree_height")]
    pub height_m: f32,
    #[serde(default)]
    pub kind: TreeKind,
    /// A model instead of the built-in shape, scaled to `height_m`.
    #[serde(default)]
    pub model: Option<String>,
}

fn default_tree_height() -> f32 {
    8.0
}

/// Street trees along roads, spaced evenly, skipping junctions, crossings,
/// street lamps, bus stops and buildings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TreeRowDef {
    /// Road ids; empty: every road with a sidewalk.
    #[serde(default)]
    pub roads: Vec<String>,
    #[serde(default)]
    pub sides: Sides,
    /// Distance between trees, m.
    #[serde(default = "default_tree_spacing")]
    pub spacing_m: f32,
    /// Shapes to pick from; empty: Round.
    #[serde(default)]
    pub kinds: Vec<TreeKind>,
    /// Height, m (min, max).
    #[serde(default = "default_tree_heights")]
    pub height_m: (f32, f32),
    /// Models to pick from instead of `kinds` (scaled to the height).
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

impl Default for TreeRowDef {
    fn default() -> Self {
        TreeRowDef {
            roads: Vec::new(),
            sides: Sides::default(),
            spacing_m: default_tree_spacing(),
            kinds: Vec::new(),
            height_m: default_tree_heights(),
            models: Vec::new(),
            seed: default_seed(),
        }
    }
}

fn default_tree_spacing() -> f32 {
    12.0
}
fn default_tree_heights() -> (f32, f32) {
    (6.0, 11.0)
}

/// How a building looks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildingStyle {
    /// Wall colours, sRGB 0–255; each building picks one.
    pub walls: Vec<(u8, u8, u8)>,
    /// Roof colour, sRGB 0–255.
    #[serde(default = "default_roof")]
    pub roof: (u8, u8, u8),
    /// Height of one storey, m.
    #[serde(default = "default_storey")]
    pub storey_m: f32,
    /// Window spacing along a wall, m (walls fit a whole number of bays).
    #[serde(default = "default_bay")]
    pub bay_m: f32,
    /// Share of each bay that is glass, 0–1 (0: blank walls).
    #[serde(default = "default_window")]
    pub window: f32,
    /// Share of windows lit at night, 0–1.
    #[serde(default = "default_lit")]
    pub lit: f32,
    /// Shop windows along the ground floor.
    #[serde(default)]
    pub shops: bool,
}

fn default_roof() -> (u8, u8, u8) {
    (96, 94, 92)
}
fn default_storey() -> f32 {
    3.2
}
fn default_bay() -> f32 {
    3.0
}
fn default_window() -> f32 {
    0.45
}
fn default_lit() -> f32 {
    0.35
}

/// A building placed by hand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildingDef {
    /// Outline on the ground, (x east, y north), m, in order round the
    /// building (either direction); at least three points.
    pub footprint: Vec<(f32, f32)>,
    /// From the ground to the roof, m.
    pub height_m: f32,
    /// A style name; absent: "plaster".
    #[serde(default)]
    pub style: Option<String>,
}

/// Which sides of a road a street row lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Sides {
    #[default]
    Both,
    /// Left of the road going from its `from` node to its `to` node.
    Left,
    Right,
}

/// A row of generated buildings along roads. Buildings stand behind the
/// sidewalk (or the carriageway edge where there is none), never on a road
/// or junction or on top of each other. The same seed gives the same street.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StreetDef {
    /// Road ids to line; empty: every road with a sidewalk.
    #[serde(default)]
    pub roads: Vec<String>,
    #[serde(default)]
    pub sides: Sides,
    /// From the sidewalk's outer edge (or the carriageway edge) to the
    /// building fronts, m.
    #[serde(default = "default_setback")]
    pub setback_m: f32,
    /// Width of each building along the street, m (min, max).
    #[serde(default = "default_frontage")]
    pub frontage_m: (f32, f32),
    /// Depth back from the street, m (min, max).
    #[serde(default = "default_depth")]
    pub depth_m: (f32, f32),
    /// Height, m (min, max); rounded to whole storeys.
    #[serde(default = "default_height")]
    pub height_m: (f32, f32),
    /// Space between neighbours, m (min, max).
    #[serde(default = "default_gap")]
    pub gap_m: (f32, f32),
    /// Styles to pick from; empty: plaster, brick and concrete.
    #[serde(default)]
    pub styles: Vec<String>,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

impl Default for StreetDef {
    fn default() -> Self {
        StreetDef {
            roads: Vec::new(),
            sides: Sides::default(),
            setback_m: default_setback(),
            frontage_m: default_frontage(),
            depth_m: default_depth(),
            height_m: default_height(),
            gap_m: default_gap(),
            styles: Vec::new(),
            seed: default_seed(),
        }
    }
}

fn default_setback() -> f32 {
    1.0
}
fn default_frontage() -> (f32, f32) {
    (10.0, 22.0)
}
fn default_depth() -> (f32, f32) {
    (10.0, 16.0)
}
fn default_height() -> (f32, f32) {
    (7.0, 22.0)
}
fn default_gap() -> (f32, f32) {
    (0.5, 4.0)
}
fn default_seed() -> u64 {
    1
}

/// The styles every game has, before its own.
pub fn builtin_styles() -> BTreeMap<String, BuildingStyle> {
    let style = |walls: &[(u8, u8, u8)], roof, storey_m, bay_m, window, lit, shops| BuildingStyle {
        walls: walls.to_vec(),
        roof,
        storey_m,
        bay_m,
        window,
        lit,
        shops,
    };
    BTreeMap::from([
        (
            "plaster".into(),
            style(
                &[
                    (214, 200, 176),
                    (196, 184, 160),
                    (222, 210, 190),
                    (190, 170, 150),
                ],
                (110, 104, 98),
                3.0,
                3.0,
                0.42,
                0.4,
                false,
            ),
        ),
        (
            "brick".into(),
            style(
                &[(150, 82, 62), (128, 70, 56), (160, 100, 76)],
                (84, 80, 78),
                3.1,
                2.8,
                0.4,
                0.35,
                false,
            ),
        ),
        (
            "concrete".into(),
            style(
                &[(170, 170, 164), (150, 150, 146), (186, 182, 172)],
                (100, 100, 100),
                3.2,
                3.2,
                0.5,
                0.3,
                false,
            ),
        ),
        (
            "office".into(),
            style(
                &[(120, 128, 136), (92, 100, 110)],
                (80, 82, 86),
                3.6,
                1.6,
                0.86,
                0.25,
                false,
            ),
        ),
        (
            "industrial".into(),
            style(
                &[(176, 178, 176), (150, 158, 164), (196, 190, 176)],
                (120, 124, 128),
                4.5,
                6.0,
                0.18,
                0.1,
                false,
            ),
        ),
        (
            "shophouse".into(),
            style(
                &[
                    (222, 196, 150),
                    (200, 170, 140),
                    (230, 220, 200),
                    (176, 196, 186),
                ],
                (108, 100, 96),
                3.1,
                3.4,
                0.4,
                0.45,
                true,
            ),
        ),
    ])
}

/// A building ready to draw (and, later, to collide with).
#[derive(Clone, Debug, PartialEq)]
pub struct Building {
    /// Outline, counter-clockwise seen from above, no repeated end point.
    pub footprint: Vec<Vec2>,
    pub height_m: f32,
    pub style: BuildingStyle,
    /// The wall colour picked from the style, sRGB.
    pub wall: (u8, u8, u8),
    /// Varies which windows are lit.
    pub seed: u8,
}

impl Building {
    pub fn centroid(&self) -> Vec2 {
        self.footprint.iter().copied().sum::<Vec2>() / self.footprint.len().max(1) as f32
    }
}

/// A tree ready to draw (its trunk is solid).
#[derive(Clone, Debug, PartialEq)]
pub struct Tree {
    pub position: Vec2,
    pub height_m: f32,
    pub kind: TreeKind,
    /// Varies crown colour and size a little.
    pub seed: u8,
    /// A model instead of the built-in shape.
    pub model: Option<String>,
}

/// Trunk width, m (also its collision box).
pub const TRUNK_M: f32 = 0.36;

impl Tree {
    /// The trunk's outline, counter-clockwise.
    pub fn trunk(&self) -> [Vec2; 4] {
        let h = TRUNK_M / 2.0;
        let p = self.position;
        [
            p + Vec2::new(-h, -h),
            p + Vec2::new(h, -h),
            p + Vec2::new(h, h),
            p + Vec2::new(-h, h),
        ]
    }
}

/// The buildings, trees and props of a scene.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scenery {
    pub buildings: Vec<Building>,
    pub trees: Vec<Tree>,
    pub props: Vec<Prop>,
    pub furniture: FurnitureDef,
}

/// A model ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    pub model: String,
    pub position: Vec2,
    /// Direction of the model's +X, rad.
    pub heading: f32,
    pub scale: f32,
    /// Half-width of its solid square, m; 0: not solid.
    pub solid_m: f32,
}

impl Prop {
    /// The solid square's outline, counter-clockwise (if solid).
    pub fn outline(&self) -> Option<[Vec2; 4]> {
        let h = self.solid_m;
        let p = self.position;
        (h > 0.0).then(|| {
            [
                p + Vec2::new(-h, -h),
                p + Vec2::new(h, -h),
                p + Vec2::new(h, h),
                p + Vec2::new(-h, h),
            ]
        })
    }
}

impl Scenery {
    /// Where the straight line from `from` to `to` first enters a building,
    /// as a share of its length (0–1), if it does. Cameras use it to stay
    /// out of walls.
    pub fn first_hit(&self, from: Vec3, to: Vec3) -> Option<f32> {
        let (a, b) = (from.truncate(), to.truncate());
        let (lo, hi) = (a.min(b), a.max(b));
        let d = b - a;
        let mut best: Option<f32> = None;
        for bld in &self.buildings {
            if from.z.min(to.z) > bld.height_m {
                continue;
            }
            let p = &bld.footprint;
            let (blo, bhi) = p.iter().fold(
                (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
                |(l, h), q| (l.min(*q), h.max(*q)),
            );
            if bhi.x < lo.x || bhi.y < lo.y || blo.x > hi.x || blo.y > hi.y {
                continue;
            }
            for i in 0..p.len() {
                let (c, e) = (p[i], p[(i + 1) % p.len()]);
                let f = e - c;
                let den = d.perp_dot(f);
                if den.abs() < 1e-9 {
                    continue;
                }
                let t = (c - a).perp_dot(f) / den;
                let u = (c - a).perp_dot(d) / den;
                if !(0.0..=1.0).contains(&t) || !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let z = from.z + (to.z - from.z) * t;
                if z <= bld.height_m && best.is_none_or(|x| t < x) {
                    best = Some(t);
                }
            }
        }
        best
    }
}

/// World resource: the current scene's scenery (read by rendering).
#[derive(Clone, Debug)]
pub struct CurrentScenery(pub std::sync::Arc<Scenery>);

/// Signed area of an outline: positive when counter-clockwise.
pub fn signed_area(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum::<f32>() * 0.5
}

/// An outline cleaned up: no repeated points or closing point,
/// counter-clockwise. `None` if fewer than three points or no area.
pub fn normalise_outline(points: &[(f32, f32)]) -> Option<Vec<Vec2>> {
    let mut p: Vec<Vec2> = Vec::with_capacity(points.len());
    for &(x, y) in points {
        let v = Vec2::new(x, y);
        if p.last().is_none_or(|l: &Vec2| l.distance(v) > 0.01) {
            p.push(v);
        }
    }
    while p.len() > 1 && p[0].distance(p[p.len() - 1]) <= 0.01 {
        p.pop();
    }
    if p.len() < 3 {
        return None;
    }
    let a = signed_area(&p);
    if a.abs() < 0.5 {
        return None;
    }
    if a < 0.0 {
        p.reverse();
    }
    Some(p)
}

/// Whether point `q` is inside outline `p` (even-odd rule).
pub fn point_in(p: &[Vec2], q: Vec2) -> bool {
    let n = p.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (p[i], p[j]);
        if (a.y > q.y) != (b.y > q.y) && q.x < (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn segments_cross(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let d1 = (b - a).perp_dot(c - a);
    let d2 = (b - a).perp_dot(d - a);
    let d3 = (d - c).perp_dot(a - c);
    let d4 = (d - c).perp_dot(b - c);
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// Whether two outlines overlap (any edges cross, or one holds the other).
pub fn outlines_overlap(a: &[Vec2], b: &[Vec2]) -> bool {
    for i in 0..a.len() {
        let (p, q) = (a[i], a[(i + 1) % a.len()]);
        for j in 0..b.len() {
            if segments_cross(p, q, b[j], b[(j + 1) % b.len()]) {
                return true;
            }
        }
    }
    point_in(b, a[0]) || point_in(a, b[0])
}

/// Room each road needs kept clear: its centreline, and how far from it
/// the carriageway and sidewalks reach.
struct Clearance {
    centre: wp_road::Polyline,
    reach_m: f32,
    min: Vec2,
    max: Vec2,
}

fn clearances(net: &RoadNetwork) -> (Vec<Clearance>, Vec<(Vec2, f32)>) {
    let roads = net
        .roads
        .iter()
        .map(|r| {
            let reach_m = r.left_m.max(r.right_m) + r.def.sidewalk_m;
            let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for p in &r.centre.points {
                min = min.min(*p);
                max = max.max(*p);
            }
            Clearance {
                centre: r.centre.clone(),
                reach_m,
                min: min - Vec2::splat(reach_m),
                max: max + Vec2::splat(reach_m),
            }
        })
        .collect();
    let junctions = net
        .junctions
        .iter()
        .map(|j| {
            let walk = j
                .arms
                .iter()
                .map(|a| net.roads[a.0].def.sidewalk_m)
                .fold(0.0, f32::max);
            let r = j
                .corners
                .iter()
                .map(|c| c.distance(j.position))
                .fold(0.0, f32::max);
            (j.position, r + walk)
        })
        .collect();
    (roads, junctions)
}

/// How far `outline` is inside the space roads and junctions need (m; 0: clear).
fn road_intrusion(
    outline: &[Vec2],
    roads: &[Clearance],
    junctions: &[(Vec2, f32)],
    margin: f32,
) -> f32 {
    let mut probes: Vec<Vec2> = Vec::with_capacity(outline.len() * 4 + 1);
    for i in 0..outline.len() {
        let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
        for k in 0..4 {
            probes.push(a.lerp(b, k as f32 / 4.0));
        }
    }
    let mut worst = 0.0f32;
    for c in roads {
        for &p in &probes {
            if p.x < c.min.x - margin
                || p.y < c.min.y - margin
                || p.x > c.max.x + margin
                || p.y > c.max.y + margin
            {
                continue;
            }
            let (_, _, d) = c.centre.project(p);
            worst = worst.max(c.reach_m + margin - d);
        }
        // A road running through a large building, past every probe.
        if c.centre.points.iter().any(|q| point_in(outline, *q)) {
            worst = worst.max(c.reach_m + margin);
        }
    }
    for &(at, r) in junctions {
        for &p in &probes {
            worst = worst.max(r + margin - p.distance(at));
        }
        if point_in(outline, at) {
            worst = worst.max(r + margin);
        }
    }
    worst
}

/// Hand-placed trees that stand on a carriageway or in a junction: indices.
pub fn trees_on_roads(def: &SceneryDef, net: &RoadNetwork) -> Vec<usize> {
    let (_, junctions) = clearances(net);
    def.trees
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            let p = Vec2::new(t.position.0, t.position.1);
            net.roads
                .iter()
                .any(|r| r.centre.project(p).2 < r.left_m.max(r.right_m) + 0.5)
                || junctions.iter().any(|(at, r)| p.distance(*at) < *r)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Hand-placed buildings that stand on a road: (index, how far in, m).
pub fn buildings_on_roads(def: &SceneryDef, net: &RoadNetwork) -> Vec<(usize, f32)> {
    let (roads, junctions) = clearances(net);
    def.buildings
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let outline = normalise_outline(&b.footprint)?;
            let d = road_intrusion(&outline, &roads, &junctions, 0.0);
            (d > 0.05).then_some((i, d))
        })
        .collect()
}

impl Scenery {
    /// Resolve styles and generate the street rows. Buildings that cannot
    /// be built (bad outline, unknown style) are skipped: `wp check` reports them.
    pub fn build(def: &SceneryDef, net: Option<&RoadNetwork>) -> Scenery {
        let mut styles = builtin_styles();
        styles.extend(def.styles.clone());
        let mut out = Vec::new();
        for (i, b) in def.buildings.iter().enumerate() {
            let Some(footprint) = normalise_outline(&b.footprint) else {
                continue;
            };
            let name = b.style.as_deref().unwrap_or("plaster");
            let Some(style) = styles.get(name) else {
                continue;
            };
            let mut h = wp_core::StableHasher::new();
            h.write_u64(i as u64);
            h.write_u64(b.height_m.to_bits() as u64);
            let seed = h.finish();
            out.push(Building {
                footprint,
                height_m: b.height_m,
                wall: pick(&style.walls, seed),
                style: style.clone(),
                seed: (seed >> 8) as u8,
            });
        }
        let mut trees: Vec<Tree> = def
            .trees
            .iter()
            .enumerate()
            .map(|(i, t)| Tree {
                position: Vec2::new(t.position.0, t.position.1),
                height_m: t.height_m,
                kind: t.kind,
                seed: (i as u32).wrapping_mul(2_654_435_761).to_be_bytes()[0],
                model: t.model.clone(),
            })
            .collect();
        let mut props: Vec<Prop> = def
            .props
            .iter()
            .map(|p| Prop {
                model: p.model.clone(),
                position: Vec2::new(p.position.0, p.position.1),
                heading: p.heading_deg.to_radians(),
                scale: p.scale,
                solid_m: p.solid_m.max(0.0),
            })
            .collect();
        if let Some(net) = net {
            let (roads, junctions) = clearances(net);
            for street in &def.streets {
                fill_street(street, net, &styles, &roads, &junctions, &mut out);
            }
            for row in &def.tree_rows {
                plant_row(row, net, &junctions, &out, &mut trees);
            }
            for row in &def.prop_rows {
                line_props(row, net, &junctions, &out, &trees, &mut props);
            }
        }
        Scenery {
            buildings: out,
            trees,
            props,
            furniture: def.furniture.clone(),
        }
    }
}

fn pick(walls: &[(u8, u8, u8)], seed: u64) -> (u8, u8, u8) {
    if walls.is_empty() {
        return (200, 190, 175);
    }
    walls[(seed % walls.len() as u64) as usize]
}

fn fill_street(
    street: &StreetDef,
    net: &RoadNetwork,
    styles: &BTreeMap<String, BuildingStyle>,
    roads: &[Clearance],
    junctions: &[(Vec2, f32)],
    out: &mut Vec<Building>,
) {
    let mut rng = SimRng::fork(street.seed, "scenery");
    let names: Vec<&str> = if street.styles.is_empty() {
        vec!["plaster", "brick", "concrete"]
    } else {
        street.styles.iter().map(String::as_str).collect()
    };
    let names: Vec<&str> = names
        .into_iter()
        .filter(|n| styles.contains_key(*n))
        .collect();
    if names.is_empty() {
        return;
    }
    let range = |rng: &mut SimRng, r: (f32, f32)| rng.range_f32(r.0.min(r.1), r.0.max(r.1));
    for road in &net.roads {
        let chosen = if street.roads.is_empty() {
            road.def.sidewalk_m > 0.0
        } else {
            street.roads.contains(&road.def.id)
        };
        if !chosen {
            continue;
        }
        let sides: &[f32] = match street.sides {
            Sides::Both => &[1.0, -1.0],
            Sides::Left => &[1.0],
            Sides::Right => &[-1.0],
        };
        for &side in sides {
            let edge = if side > 0.0 {
                road.left_m
            } else {
                road.right_m
            };
            let offset = side * (edge + road.def.sidewalk_m + street.setback_m.max(0.0));
            let line = road.centre.offset(offset);
            let len = line.length();
            let mut s = range(&mut rng, street.gap_m);
            while s < len {
                let w = range(&mut rng, street.frontage_m).max(3.0);
                if s + w > len {
                    break;
                }
                let depth = range(&mut rng, street.depth_m).max(3.0);
                let height = range(&mut rng, street.height_m).max(3.0);
                let style = styles[names[(rng.next_u64() % names.len() as u64) as usize]].clone();
                let wall_seed = rng.next_u64();
                let (a, b) = (line.sample(s).0, line.sample(s + w).0);
                let along = (b - a).normalize_or(Vec2::X);
                let away = Vec2::new(-along.y, along.x) * side;
                let mut footprint = vec![a, b, b + away * depth, a + away * depth];
                if signed_area(&footprint) < 0.0 {
                    footprint.reverse();
                }
                let storeys = (height / style.storey_m).round().max(1.0);
                let clear = road_intrusion(&footprint, roads, junctions, 0.3) <= 0.0
                    && !out.iter().any(|o| {
                        o.centroid().distance(footprint[0]) < 80.0
                            && outlines_overlap(&o.footprint, &footprint)
                    });
                if clear {
                    out.push(Building {
                        height_m: storeys * style.storey_m + 0.4,
                        wall: pick(&style.walls, wall_seed),
                        seed: (wall_seed >> 8) as u8,
                        style,
                        footprint,
                    });
                }
                s += w + range(&mut rng, street.gap_m);
            }
        }
    }
}

/// Street trees stand this far in from the kerb, m.
const KERB_TREE_M: f32 = 1.2;
/// And this far beyond a junction's corners, m.
const JUNCTION_TREE_M: f32 = 14.0;

fn plant_row(
    row: &TreeRowDef,
    net: &RoadNetwork,
    junctions: &[(Vec2, f32)],
    buildings: &[Building],
    out: &mut Vec<Tree>,
) {
    let mut rng = SimRng::fork(row.seed, "trees");
    let kinds = if row.kinds.is_empty() {
        vec![TreeKind::Round]
    } else {
        row.kinds.clone()
    };
    let spacing = row.spacing_m.max(3.0);
    let (lo, hi) = (
        row.height_m.0.min(row.height_m.1),
        row.height_m.0.max(row.height_m.1),
    );
    for road in &net.roads {
        let chosen = if row.roads.is_empty() {
            road.def.sidewalk_m > 0.0
        } else {
            row.roads.contains(&road.def.id)
        };
        if !chosen {
            continue;
        }
        let sides: &[f32] = match row.sides {
            Sides::Both => &[1.0, -1.0],
            Sides::Left => &[1.0],
            Sides::Right => &[-1.0],
        };
        for &side in sides {
            let edge = if side > 0.0 {
                road.left_m
            } else {
                road.right_m
            };
            // In the sidewalk, clear of overhanging bodies at the kerb, or on the verge.
            let out_m = if road.def.sidewalk_m >= 1.8 {
                KERB_TREE_M
            } else {
                road.def.sidewalk_m + 1.8
            };
            let line = road.centre.offset(side * (edge + out_m));
            let len = line.length();
            let mut s = spacing * 0.5 + rng.range_f32(0.0, spacing * 0.3);
            while s < len {
                let p = line.sample(s).0;
                let height_m = rng.range_f32(lo, hi.max(lo));
                let kind = kinds[(rng.next_u64() % kinds.len() as u64) as usize];
                let seed = (rng.next_u64() >> 24) as u8;
                // Drawn only when the row has models (so rows without keep
                // their old random sequence).
                let model = (!row.models.is_empty()).then(|| {
                    row.models[(rng.next_u64() % row.models.len() as u64) as usize].clone()
                });
                s += spacing;
                // Long vehicles cut corners: keep well back from junctions.
                let clear = junctions
                    .iter()
                    .all(|(at, r)| p.distance(*at) > r + JUNCTION_TREE_M)
                    && net.lamps.iter().all(|l| l.position.distance(p) > 3.0)
                    && net.stops.iter().all(|st| st.kerb.distance(p) > 10.0)
                    && buildings.iter().all(|b| {
                        b.centroid().distance(p) > 60.0
                            || (!point_in(&b.footprint, p)
                                && outline_distance(&b.footprint, p) > 1.5)
                    })
                    && net
                        .roads
                        .iter()
                        .all(|r| r.centre.project(p).2 > r.left_m.max(r.right_m) + 0.4)
                    && out.iter().all(|t| t.position.distance(p) > 2.0);
                if clear {
                    out.push(Tree {
                        position: p,
                        height_m,
                        kind,
                        seed,
                        model,
                    });
                }
            }
        }
    }
}

/// Props stay this far beyond a junction's corners (clear of crossings), m.
const JUNCTION_PROP_M: f32 = 6.0;

fn line_props(
    row: &PropRowDef,
    net: &RoadNetwork,
    junctions: &[(Vec2, f32)],
    buildings: &[Building],
    trees: &[Tree],
    out: &mut Vec<Prop>,
) {
    if row.models.is_empty() {
        return;
    }
    let mut rng = SimRng::fork(row.seed, "props");
    let spacing = row.spacing_m.max(2.0);
    for road in &net.roads {
        let chosen = if row.roads.is_empty() {
            road.def.sidewalk_m > 0.0
        } else {
            row.roads.contains(&road.def.id)
        };
        if !chosen {
            continue;
        }
        let sides: &[f32] = match row.sides {
            Sides::Both => &[1.0, -1.0],
            Sides::Left => &[1.0],
            Sides::Right => &[-1.0],
        };
        for &side in sides {
            let edge = if side > 0.0 {
                road.left_m
            } else {
                road.right_m
            };
            let line = road.centre.offset(side * (edge + row.kerb_m));
            let len = line.length();
            let mut s = spacing * 0.5 + rng.range_f32(0.0, spacing * 0.4);
            while s < len {
                let (p, dir) = line.sample(s);
                let model = row.models[(rng.next_u64() % row.models.len() as u64) as usize].clone();
                s += spacing;
                // +X towards the road: the road is to the right of `dir` on
                // the left side and to its left on the right side.
                let to_road = Vec2::new(dir.y, -dir.x) * side;
                let clear = junctions
                    .iter()
                    .all(|(at, r)| p.distance(*at) > r + JUNCTION_PROP_M)
                    && net.lamps.iter().all(|l| l.position.distance(p) > 2.0)
                    && net.stops.iter().all(|st| st.kerb.distance(p) > 8.0)
                    && trees.iter().all(|t| t.position.distance(p) > 1.5)
                    && buildings.iter().all(|b| {
                        b.centroid().distance(p) > 60.0
                            || (!point_in(&b.footprint, p)
                                && outline_distance(&b.footprint, p) > 0.8)
                    })
                    && net
                        .roads
                        .iter()
                        .all(|r| r.centre.project(p).2 > r.left_m.max(r.right_m) + 0.2)
                    && out.iter().all(|q| q.position.distance(p) > 1.5);
                if clear {
                    out.push(Prop {
                        model,
                        position: p,
                        heading: to_road.y.atan2(to_road.x),
                        scale: 1.0,
                        solid_m: row.solid_m.max(0.0),
                    });
                }
            }
        }
    }
}

/// Every model path the scenery refers to (for checking they exist).
impl SceneryDef {
    pub fn model_paths(&self) -> Vec<&str> {
        let mut v: Vec<&str> = Vec::new();
        v.extend(self.props.iter().map(|p| p.model.as_str()));
        for r in &self.prop_rows {
            v.extend(r.models.iter().map(|m| m.as_str()));
        }
        v.extend(self.trees.iter().filter_map(|t| t.model.as_deref()));
        for r in &self.tree_rows {
            v.extend(r.models.iter().map(|m| m.as_str()));
        }
        if let Some(l) = &self.furniture.street_lamp {
            v.push(&l.model);
        }
        v.extend(self.furniture.bus_stop.as_deref());
        v.extend(self.furniture.people.iter().map(|m| m.as_str()));
        v
    }
}

/// Distance from `p` to the nearest edge of an outline, m.
fn outline_distance(outline: &[Vec2], p: Vec2) -> f32 {
    let n = outline.len();
    (0..n)
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            p.distance(a + ab * t)
        })
        .fold(f32::MAX, f32::min)
}

impl SceneryDef {
    pub fn validate(file: &Loaded<SceneryDef>) -> Vec<Diagnostic> {
        let def = &file.value;
        let mut out = Vec::new();
        let mut styles = builtin_styles();
        styles.extend(def.styles.clone());
        let known = || styles.keys().cloned().collect::<Vec<_>>().join(", ");
        for (name, s) in &def.styles {
            let bad = if s.walls.is_empty() {
                Some("needs at least one wall colour".to_string())
            } else if !(1.5..=10.0).contains(&s.storey_m) {
                Some(format!("storey_m {} is outside 1.5–10", s.storey_m))
            } else if !(0.5..=10.0).contains(&s.bay_m) {
                Some(format!("bay_m {} is outside 0.5–10", s.bay_m))
            } else if !(0.0..=1.0).contains(&s.window) || !(0.0..=1.0).contains(&s.lit) {
                Some("window and lit are shares from 0 to 1".to_string())
            } else {
                None
            };
            if let Some(why) = bad {
                out.push(
                    file.error_at("WP-B005", name, format!("style `{name}` {why}"))
                        .help(
                            "walls: [(r, g, b)], storey_m 1.5–10, bay_m 0.5–10, window and lit 0–1",
                        ),
                );
            }
        }
        for (i, b) in def.buildings.iter().enumerate() {
            if normalise_outline(&b.footprint).is_none() {
                out.push(
                    file.error_at(
                        "WP-B001",
                        "buildings",
                        format!("building {i} has no area: its footprint needs three or more distinct points"),
                    )
                    .help("list the outline's corners in order round the building, (x, y) in metres"),
                );
            }
            if !(b.height_m > 0.0 && b.height_m <= 600.0) {
                out.push(
                    file.error_at(
                        "WP-B003",
                        "height_m",
                        format!("building {i} has height_m {}", b.height_m),
                    )
                    .help("use a height above 0 and at most 600 m"),
                );
            }
            if let Some(s) = &b.style
                && !styles.contains_key(s)
            {
                out.push(
                    file.error_at(
                        "WP-B002",
                        "style",
                        format!("building {i} uses unknown style `{s}`"),
                    )
                    .help(format!("known styles: {}", known())),
                );
            }
        }
        for (i, t) in def.trees.iter().enumerate() {
            if !(t.height_m > 1.0 && t.height_m <= 80.0) {
                out.push(
                    file.error_at(
                        "WP-B008",
                        "trees",
                        format!("tree {i} has height_m {}", t.height_m),
                    )
                    .help("tree heights run from 1 to 80 m"),
                );
            }
        }
        for r in &def.tree_rows {
            if !(r.spacing_m >= 3.0)
                || r.height_m.0 <= 1.0
                || r.height_m.1 < r.height_m.0
                || r.height_m.1 > 80.0
            {
                out.push(
                    file.error_at(
                        "WP-B008",
                        "tree_rows",
                        "a tree row needs spacing_m ≥ 3 and height_m (min, max) within 1–80 m",
                    )
                    .help("for example spacing_m: 12.0, height_m: (6.0, 11.0)"),
                );
            }
        }
        let props_ok = def
            .props
            .iter()
            .all(|p| p.scale > 0.0 && p.scale <= 100.0 && (0.0..=20.0).contains(&p.solid_m))
            && def.prop_rows.iter().all(|r| {
                r.spacing_m >= 2.0
                    && (0.0..=10.0).contains(&r.kerb_m)
                    && (0.0..=20.0).contains(&r.solid_m)
                    && !r.models.is_empty()
            });
        if !props_ok {
            out.push(
                file.error_at("WP-B010", "props", "a prop or prop row has values out of range")
                    .help("props: scale 0–100, solid_m 0–20; prop_rows: models listed, spacing_m ≥ 2, kerb_m 0–10"),
            );
        }
        for st in &def.streets {
            for s in &st.styles {
                if !styles.contains_key(s) {
                    out.push(
                        file.error_at(
                            "WP-B002",
                            "styles",
                            format!("street uses unknown style `{s}`"),
                        )
                        .help(format!("known styles: {}", known())),
                    );
                }
            }
            let ranges = [st.frontage_m, st.depth_m, st.height_m];
            if ranges.iter().any(|r| r.0 <= 0.0 || r.1 < r.0)
                || st.gap_m.0 < 0.0
                || st.gap_m.1 < st.gap_m.0
            {
                out.push(
                    file.error_at(
                        "WP-B005",
                        "streets",
                        "a street's size ranges must be (min, max) with 0 < min ≤ max",
                    )
                    .help("for example frontage_m: (10.0, 22.0), gap_m: (0.5, 4.0)"),
                );
            }
        }
        out
    }
}

// ---- Meshes -------------------------------------------------------------------

/// Building geometry as plain data. Per vertex: position, normal, facade
/// coordinates (m along the wall, m up), colour (sRGB; alpha varies which
/// windows are lit) and the facade pattern: storey height and bay width
/// (m / 10 × 255), glass share (× 255), lit share (× 127, + 128 for shop
/// windows on the ground floor). A zero storey height means no windows (roofs).
#[derive(Clone, Debug, Default)]
pub struct SceneryMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[u8; 4]>,
    pub facades: Vec<[u8; 4]>,
    pub indices: Vec<u32>,
}

impl SceneryMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    fn vertex(&mut self, p: Vec3, n: Vec3, uv: [f32; 2], color: [u8; 4], facade: [u8; 4]) -> u32 {
        self.positions.push(p.to_array());
        self.normals.push(n.to_array());
        self.uvs.push(uv);
        self.colors.push(color);
        self.facades.push(facade);
        self.positions.len() as u32 - 1
    }
}

fn quantise(x: f32, scale: f32) -> u8 {
    (x * scale).round().clamp(0.0, 255.0) as u8
}

/// Add one building to `mesh`, positions relative to `origin`.
pub fn building_mesh(mesh: &mut SceneryMesh, b: &Building, origin: Vec2) {
    let s = &b.style;
    let pts: Vec<Vec2> = b.footprint.iter().map(|p| *p - origin).collect();
    let n = pts.len();
    let h = b.height_m;
    for i in 0..n {
        let (a, c) = (pts[i], pts[(i + 1) % n]);
        let len = a.distance(c);
        if len < 0.01 {
            continue;
        }
        let along = (c - a) / len;
        let out = Vec3::new(along.y, -along.x, 0.0);
        // A whole number of bays per wall, so no window is cut by a corner.
        let bays = (len / s.bay_m).round();
        let (bay, window) = if bays >= 1.0 && s.window > 0.0 {
            (len / bays, s.window)
        } else {
            (s.bay_m, 0.0)
        };
        let facade = [
            quantise(s.storey_m, 25.5),
            quantise(bay.min(10.0), 25.5),
            quantise(window, 255.0),
            quantise(s.lit, 127.0) + if s.shops { 128 } else { 0 },
        ];
        let color = [
            b.wall.0,
            b.wall.1,
            b.wall.2,
            b.seed.wrapping_add((i as u8).wrapping_mul(37)),
        ];
        let v0 = mesh.vertex(a.extend(0.0), out, [0.0, 0.0], color, facade);
        let v1 = mesh.vertex(c.extend(0.0), out, [len, 0.0], color, facade);
        let v2 = mesh.vertex(c.extend(h), out, [len, h], color, facade);
        let v3 = mesh.vertex(a.extend(h), out, [0.0, h], color, facade);
        mesh.indices.extend_from_slice(&[v0, v1, v2, v0, v2, v3]);
    }
    let roof = [s.roof.0, s.roof.1, s.roof.2, 255];
    let base = mesh.positions.len() as u32;
    for p in &pts {
        mesh.vertex(p.extend(h), Vec3::Z, [p.x, p.y], roof, [0; 4]);
    }
    for [a, b, c] in triangulate(&pts) {
        mesh.indices
            .extend_from_slice(&[base + a as u32, base + b as u32, base + c as u32]);
    }
}

/// Ear-clipping triangulation of a counter-clockwise simple polygon.
pub fn triangulate(p: &[Vec2]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..p.len()).collect();
    let mut out = Vec::with_capacity(p.len().saturating_sub(2));
    let mut guard = 0;
    while idx.len() > 3 && guard < p.len() * p.len() {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (p[ia], p[ib], p[ic]);
            if (b - a).perp_dot(c - b) <= 1e-6 {
                continue; // reflex or flat corner
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let q = p[j];
                (b - a).perp_dot(q - a) > 0.0
                    && (c - b).perp_dot(q - b) > 0.0
                    && (a - c).perp_dot(q - c) > 0.0
            });
            if !inside {
                out.push([ia, ib, ic]);
                idx.remove(k);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break; // not simple: fan the rest
        }
    }
    for k in 1..idx.len().saturating_sub(1) {
        out.push([idx[0], idx[k], idx[k + 1]]);
    }
    out
}

/// A square of the scenery for streaming; positions relative to `centre`.
#[derive(Clone, Debug, Default)]
pub struct SceneryTile {
    pub key: (i32, i32),
    pub centre: [f32; 2],
    pub mesh: SceneryMesh,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Buildings cut into `tile_m` squares (each in the tile of its centre), in key order.
pub fn build_tiles(scenery: &Scenery, tile_m: f32) -> Vec<SceneryTile> {
    let mut tiles: BTreeMap<(i32, i32), SceneryTile> = BTreeMap::new();
    for b in &scenery.buildings {
        let c = b.centroid();
        let key = ((c.x / tile_m).floor() as i32, (c.y / tile_m).floor() as i32);
        let tile = tiles.entry(key).or_insert_with(|| SceneryTile {
            key,
            centre: [(key.0 as f32 + 0.5) * tile_m, (key.1 as f32 + 0.5) * tile_m],
            min: [f32::MAX; 3],
            max: [f32::MIN; 3],
            ..Default::default()
        });
        let origin = Vec2::from(tile.centre);
        let from = tile.mesh.positions.len();
        building_mesh(&mut tile.mesh, b, origin);
        for p in &tile.mesh.positions[from..] {
            tile.min = std::array::from_fn(|a| tile.min[a].min(p[a]));
            tile.max = std::array::from_fn(|a| tile.max[a].max(p[a]));
        }
    }
    tiles.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn town() -> RoadNetwork {
        let def: wp_road::RoadNetworkDef = wp_data::parse_ron(
            "town.road.ron",
            include_str!("../../../examples/sandbox/roads/town.road.ron"),
        )
        .unwrap();
        RoadNetwork::build(&def)
    }

    #[test]
    fn outlines_are_counter_clockwise_and_triangulate() {
        // An L shape, given clockwise with a closing point.
        let l = [
            (0.0, 0.0),
            (0.0, 10.0),
            (5.0, 10.0),
            (5.0, 5.0),
            (10.0, 5.0),
            (10.0, 0.0),
            (0.0, 0.0),
        ];
        let p = normalise_outline(&l).unwrap();
        assert_eq!(p.len(), 6);
        assert!(signed_area(&p) > 0.0);
        let tris = triangulate(&p);
        assert_eq!(tris.len(), 4);
        let area: f32 = tris
            .iter()
            .map(|t| (p[t[1]] - p[t[0]]).perp_dot(p[t[2]] - p[t[0]]) * 0.5)
            .sum();
        assert!((area - 75.0).abs() < 1e-3, "area {area}");
        assert!(normalise_outline(&[(0.0, 0.0), (1.0, 0.0), (0.0, 0.0)]).is_none());
    }

    #[test]
    fn street_rows_stay_off_roads_and_each_other() {
        let net = town();
        let def = SceneryDef {
            name: "t".into(),
            streets: vec![StreetDef {
                roads: Vec::new(),
                sides: Sides::Both,
                setback_m: 1.0,
                frontage_m: default_frontage(),
                depth_m: default_depth(),
                height_m: default_height(),
                gap_m: default_gap(),
                styles: Vec::new(),
                seed: 7,
            }],
            ..Default::default()
        };
        let s = Scenery::build(&def, Some(&net));
        assert!(s.buildings.len() > 20, "{} buildings", s.buildings.len());
        let (roads, junctions) = clearances(&net);
        for (i, b) in s.buildings.iter().enumerate() {
            assert!(road_intrusion(&b.footprint, &roads, &junctions, 0.0) <= 0.0);
            for o in &s.buildings[..i] {
                assert!(!outlines_overlap(&o.footprint, &b.footprint));
            }
            // Heights are whole storeys plus a parapet.
            let storeys = (b.height_m - 0.4) / b.style.storey_m;
            assert!((storeys - storeys.round()).abs() < 1e-3);
        }
        // Same seed, same street.
        assert_eq!(Scenery::build(&def, Some(&net)), s);
    }

    #[test]
    fn tiles_hold_walls_and_roofs() {
        let def = SceneryDef {
            name: "t".into(),
            buildings: vec![BuildingDef {
                footprint: vec![(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)],
                height_m: 12.0,
                style: Some("brick".into()),
            }],
            ..Default::default()
        };
        let s = Scenery::build(&def, None);
        let tiles = build_tiles(&s, 400.0);
        assert_eq!(tiles.len(), 1);
        let m = &tiles[0].mesh;
        assert_eq!(m.triangle_count(), 4 * 2 + 2);
        // Walls face outwards: the wall along y = 0 faces south.
        let south = m.normals.iter().position(|n| n[1] < -0.9).unwrap();
        assert_eq!(m.facades[south][0], quantise(3.1, 25.5));
        // Roof vertices carry no windows.
        let roof = m.normals.iter().position(|n| n[2] > 0.9).unwrap();
        assert_eq!(m.facades[roof], [0; 4]);
        // Every triangle winds counter-clockwise seen from outside.
        for t in m.indices.chunks(3) {
            let p = |i: u32| Vec3::from(m.positions[i as usize]);
            let face = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            assert!(face.dot(Vec3::from(m.normals[t[0] as usize])) > 0.0);
        }
    }

    #[test]
    fn tree_rows_keep_clear_of_roads_lamps_and_buildings() {
        let net = town();
        let def = SceneryDef {
            name: "t".into(),
            streets: vec![StreetDef {
                roads: Vec::new(),
                sides: Sides::Both,
                setback_m: 1.0,
                frontage_m: default_frontage(),
                depth_m: default_depth(),
                height_m: default_height(),
                gap_m: default_gap(),
                styles: Vec::new(),
                seed: 7,
            }],
            tree_rows: vec![TreeRowDef {
                roads: Vec::new(),
                sides: Sides::Both,
                spacing_m: 10.0,
                kinds: vec![TreeKind::Round, TreeKind::Palm],
                height_m: (6.0, 10.0),
                models: Vec::new(),
                seed: 2,
            }],
            ..Default::default()
        };
        let s = Scenery::build(&def, Some(&net));
        assert!(s.trees.len() > 30, "{} trees", s.trees.len());
        for t in &s.trees {
            for r in &net.roads {
                assert!(r.centre.project(t.position).2 > r.left_m.max(r.right_m));
            }
            assert!(
                net.lamps
                    .iter()
                    .all(|l| l.position.distance(t.position) > 3.0)
            );
            assert!(
                s.buildings
                    .iter()
                    .all(|b| !point_in(&b.footprint, t.position))
            );
        }
        assert!(s.trees.iter().any(|t| t.kind == TreeKind::Palm));
        assert_eq!(Scenery::build(&def, Some(&net)), s, "same seed, same trees");
    }

    #[test]
    fn prop_rows_line_the_kerb_facing_the_road() {
        let net = town();
        let def = SceneryDef {
            name: "t".into(),
            tree_rows: vec![TreeRowDef::default()],
            prop_rows: vec![PropRowDef {
                models: vec![
                    "assets/props/bin.glb".into(),
                    "assets/props/bench.glb".into(),
                ],
                roads: Vec::new(),
                sides: Sides::Both,
                spacing_m: 15.0,
                kerb_m: 0.6,
                solid_m: 0.3,
                seed: 4,
            }],
            ..Default::default()
        };
        let s = Scenery::build(&def, Some(&net));
        assert!(s.props.len() > 20, "{} props", s.props.len());
        for p in &s.props {
            // On the sidewalk, off every carriageway.
            let (road, _) = net
                .roads
                .iter()
                .map(|r| (r, r.centre.project(p.position).2))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            let (at, _, d) = road.centre.project(p.position);
            assert!(d > road.left_m.max(road.right_m), "{p:?} on the road");
            // +X faces the road: towards the nearest point of its centre line.
            let to_road = (road.centre.sample(at).0 - p.position).normalize();
            let facing = Vec2::new(p.heading.cos(), p.heading.sin());
            assert!(facing.dot(to_road) > 0.9, "{p:?} faces {facing:?}");
            assert!(
                s.trees
                    .iter()
                    .all(|t| t.position.distance(p.position) > 1.5)
            );
            let corner = p.outline().unwrap()[2] - p.position;
            assert!((corner - Vec2::splat(0.3)).length() < 1e-3, "{corner:?}");
        }
        assert!(s.props.iter().any(|p| p.model.ends_with("bench.glb")));
        assert_eq!(Scenery::build(&def, Some(&net)), s, "same seed, same props");
        // Adding props leaves the trees where they were.
        let plain = SceneryDef {
            prop_rows: Vec::new(),
            ..def.clone()
        };
        assert_eq!(Scenery::build(&plain, Some(&net)).trees, s.trees);
    }

    #[test]
    fn lines_of_sight_stop_at_walls() {
        let s = Scenery::build(
            &SceneryDef {
                name: "t".into(),
                buildings: vec![BuildingDef {
                    footprint: vec![(10.0, -5.0), (20.0, -5.0), (20.0, 5.0), (10.0, 5.0)],
                    height_m: 8.0,
                    style: None,
                }],
                ..Default::default()
            },
            None,
        );
        let t = s
            .first_hit(Vec3::new(0.0, 0.0, 2.0), Vec3::new(20.0, 0.0, 2.0))
            .unwrap();
        assert!((t - 0.5).abs() < 1e-5);
        assert!(
            s.first_hit(Vec3::new(0.0, 0.0, 9.0), Vec3::new(20.0, 0.0, 9.0))
                .is_none(),
            "over the roof"
        );
        assert!(
            s.first_hit(Vec3::new(0.0, 8.0, 2.0), Vec3::new(20.0, 8.0, 2.0))
                .is_none(),
            "beside it"
        );
    }

    #[test]
    fn hand_placed_buildings_on_roads_are_found() {
        let net = town();
        let def = SceneryDef {
            name: "t".into(),
            buildings: vec![
                // Across Station Avenue.
                BuildingDef {
                    footprint: vec![(-80.0, -5.0), (-70.0, -5.0), (-70.0, 5.0), (-80.0, 5.0)],
                    height_m: 10.0,
                    style: None,
                },
                // Well clear of every road.
                BuildingDef {
                    footprint: vec![(-80.0, 40.0), (-70.0, 40.0), (-70.0, 50.0), (-80.0, 50.0)],
                    height_m: 10.0,
                    style: None,
                },
            ],
            ..Default::default()
        };
        let on = buildings_on_roads(&def, &net);
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].0, 0);
    }
}
