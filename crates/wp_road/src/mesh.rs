//! Road meshes: plain vertex data (no graphics dependency) that the renderer
//! uploads once. Heights keep the layers apart: asphalt 2 cm above the ground,
//! markings 3.5 cm, sidewalk tops 17 cm.

use crate::geometry::Polyline;
use crate::network::{RoadGeometry, RoadNetwork};
use glam::{Vec2, Vec3};

const ASPHALT_Z: f32 = 0.02;
const MARKING_Z: f32 = 0.035;
const CURB_Z: f32 = 0.17;
const LINE_W: f32 = 0.13;
const DASH_ON: f32 = 3.0;
const DASH_OFF: f32 = 6.0;

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    fn vertex(&mut self, p: Vec3, n: Vec3) -> u32 {
        self.positions.push(p.to_array());
        self.normals.push(n.to_array());
        self.positions.len() as u32 - 1
    }

    /// A flat strip between two equally long side lines, facing up.
    fn strip(&mut self, left: &[Vec2], right: &[Vec2], z: f32) {
        let n = left.len().min(right.len());
        if n < 2 {
            return;
        }
        let base = self.positions.len() as u32;
        for i in 0..n {
            self.vertex(left[i].extend(z), Vec3::Z);
            self.vertex(right[i].extend(z), Vec3::Z);
        }
        for i in 0..n as u32 - 1 {
            let (l0, r0, l1, r1) = (
                base + 2 * i,
                base + 2 * i + 1,
                base + 2 * i + 2,
                base + 2 * i + 3,
            );
            // Counter-clockwise seen from above.
            self.indices.extend_from_slice(&[l0, r0, r1, l0, r1, l1]);
        }
    }

    /// A vertical face from `a` to `b`, from z0 up to z1, facing `out`.
    fn wall(&mut self, a: Vec2, b: Vec2, z0: f32, z1: f32, out: Vec2) {
        let n = out.extend(0.0);
        let (v0, v1, v2, v3) = (
            self.vertex(a.extend(z0), n),
            self.vertex(b.extend(z0), n),
            self.vertex(b.extend(z1), n),
            self.vertex(a.extend(z1), n),
        );
        let face = (b - a).extend(0.0).cross(Vec3::Z).dot(n);
        if face >= 0.0 {
            self.indices.extend_from_slice(&[v0, v1, v2, v0, v2, v3]);
        } else {
            self.indices.extend_from_slice(&[v0, v2, v1, v0, v3, v2]);
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Split triangles until no edge is longer than `max_edge` (measured
    /// across the ground). Each round splits every long edge at its middle,
    /// once for all triangles sharing it, and each triangle into 2, 3 or 4 by
    /// how many of its edges were split, so no cracks open.
    pub fn refine(&mut self, max_edge: f32) {
        use std::collections::BTreeMap;
        let flat_len = |p: [f32; 3], q: [f32; 3]| Vec2::new(p[0] - q[0], p[1] - q[1]).length();
        for _ in 0..12 {
            let mut mid: BTreeMap<(u32, u32), u32> = BTreeMap::new();
            let indices = std::mem::take(&mut self.indices);
            for t in indices.chunks(3) {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    let key = (a.min(b), a.max(b));
                    if !mid.contains_key(&key)
                        && flat_len(self.positions[a as usize], self.positions[b as usize])
                            > max_edge
                    {
                        let (pa, pb) = (
                            Vec3::from(self.positions[a as usize]),
                            Vec3::from(self.positions[b as usize]),
                        );
                        let n = (Vec3::from(self.normals[a as usize])
                            + Vec3::from(self.normals[b as usize]))
                        .normalize_or(Vec3::Z);
                        let m = self.vertex((pa + pb) * 0.5, n);
                        mid.insert(key, m);
                    }
                }
            }
            if mid.is_empty() {
                self.indices = indices;
                return;
            }
            let split = |a: u32, b: u32| mid.get(&(a.min(b), a.max(b))).copied();
            let mut out = Vec::with_capacity(indices.len() * 2);
            for t in indices.chunks(3) {
                // Rotate so the split edges come first: ab, then bc, then ca.
                let tri = [t[0], t[1], t[2]];
                let marks: Vec<bool> = (0..3)
                    .map(|k| split(tri[k], tri[(k + 1) % 3]).is_some())
                    .collect();
                let count = marks.iter().filter(|m| **m).count();
                let r = match count {
                    0 => {
                        out.extend_from_slice(&tri);
                        continue;
                    }
                    // One split edge: rotate it to ab. Two: rotate the unsplit one to ca.
                    1 => marks.iter().position(|m| *m).expect("one"),
                    2 => (marks.iter().position(|m| !*m).expect("one unsplit") + 1) % 3,
                    _ => 0,
                };
                let (a, b, c) = (tri[r], tri[(r + 1) % 3], tri[(r + 2) % 3]);
                match count {
                    1 => {
                        let m = split(a, b).expect("split");
                        out.extend_from_slice(&[a, m, c, m, b, c]);
                    }
                    2 => {
                        let (m1, m2) = (split(a, b).expect("ab"), split(b, c).expect("bc"));
                        out.extend_from_slice(&[m1, b, m2, a, m1, m2, a, m2, c]);
                    }
                    _ => {
                        let (ab, bc, ca) = (
                            split(a, b).expect("ab"),
                            split(b, c).expect("bc"),
                            split(c, a).expect("ca"),
                        );
                        out.extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
                    }
                }
            }
            self.indices = out;
        }
    }
}

/// Longest triangle edge when road meshes are laid on hilly ground, m.
pub const DRAPE_EDGE_M: f32 = 2.0;

/// The three road layers, each drawn with its own material.
#[derive(Clone, Debug, Default)]
pub struct RoadMeshes {
    pub asphalt: Mesh,
    pub markings: Mesh,
    pub sidewalks: Mesh,
}

impl RoadMeshes {
    /// Lay the meshes on hilly ground: every vertex rises by the ground's
    /// `height` under it, and upward faces tilt with the ground's `gradient`
    /// (rise per metre in x and y).
    /// Long triangles are split first (no edge over [`DRAPE_EDGE_M`]) so the
    /// surface follows the ground between its corners.
    pub fn drape(&mut self, height: impl Fn(Vec2) -> f32, gradient: impl Fn(Vec2) -> Vec2) {
        for m in [&mut self.asphalt, &mut self.markings, &mut self.sidewalks] {
            m.refine(DRAPE_EDGE_M);
        }
        for m in [&mut self.asphalt, &mut self.markings, &mut self.sidewalks] {
            for (p, n) in m.positions.iter_mut().zip(&mut m.normals) {
                let xy = Vec2::new(p[0], p[1]);
                p[2] += height(xy);
                if n[2] > 0.9 {
                    let g = gradient(xy);
                    *n = Vec3::new(-g.x, -g.y, 1.0).normalize().to_array();
                }
            }
        }
    }
}

/// A painted line `offset` metres left of `centre`, solid or dashed.
fn line(mesh: &mut Mesh, centre: &Polyline, offset: f32, dashed: bool) {
    paint(mesh, centre.offset(offset), dashed);
}

/// A painted line along `path`, solid or dashed.
fn paint(mesh: &mut Mesh, path: Polyline, dashed: bool) {
    let len = path.length();
    if len < 0.5 {
        return;
    }
    let pieces: Vec<(f32, f32)> = if dashed {
        let mut v = Vec::new();
        let mut s = DASH_OFF / 2.0;
        while s < len {
            v.push((s, (s + DASH_ON).min(len)));
            s += DASH_ON + DASH_OFF;
        }
        v
    } else {
        vec![(0.0, len)]
    };
    for (a, b) in pieces {
        let piece = path.slice(a, b).resample(1.0);
        let left: Vec<Vec2> = piece.offset(LINE_W / 2.0).points;
        let right: Vec<Vec2> = piece.offset(-LINE_W / 2.0).points;
        mesh.strip(&left, &right, MARKING_Z);
    }
}

pub fn build_meshes(net: &RoadNetwork) -> RoadMeshes {
    let mut m = RoadMeshes::default();
    for road in &net.roads {
        let c = road.centre.resample(1.0);
        if c.is_empty() {
            continue;
        }
        if road.has_tapers() {
            tapered_road(&mut m, road, &c);
            continue;
        }
        let left = c.offset(road.left_m);
        let right = c.offset(-road.right_m);
        m.asphalt.strip(&left.points, &right.points, ASPHALT_Z);

        // Markings: centre line between the directions, lane lines, edge lines.
        let d = &road.def;
        let w = d.lane_width_m;
        let two_way = d.lanes_forward > 0 && d.lanes_backward > 0;
        if two_way {
            if d.total_lanes() >= 4 {
                line(&mut m.markings, &c, 0.12, false);
                line(&mut m.markings, &c, -0.12, false);
            } else {
                line(&mut m.markings, &c, 0.0, true);
            }
        }
        if two_way {
            // Lane lines within each direction (the centre line sits at 0).
            let left_lanes = (road.left_m / w).round() as i32;
            let right_lanes = (road.right_m / w).round() as i32;
            for k in 1..left_lanes {
                line(&mut m.markings, &c, k as f32 * w, true);
            }
            for k in 1..right_lanes {
                line(&mut m.markings, &c, -(k as f32) * w, true);
            }
        } else {
            // One-way: the carriageway is centred, lanes measured from the left edge.
            for k in 1..d.total_lanes() {
                line(&mut m.markings, &c, road.left_m - k as f32 * w, true);
            }
        }
        line(&mut m.markings, &c, road.left_m - 0.3, false);
        line(&mut m.markings, &c, -road.right_m + 0.3, false);

        // Sidewalks: a raised slab beyond each edge with a curb face.
        if d.sidewalk_m > 0.0 {
            for (edge, side) in [(road.left_m, 1.0f32), (-road.right_m, -1.0f32)] {
                let inner = c.offset(edge);
                let outer = c.offset(edge + side * d.sidewalk_m);
                if side > 0.0 {
                    m.sidewalks.strip(&outer.points, &inner.points, CURB_Z);
                } else {
                    m.sidewalks.strip(&inner.points, &outer.points, CURB_Z);
                }
                for i in 1..inner.points.len() {
                    let (a, b) = (inner.points[i - 1], inner.points[i]);
                    let t = (b - a).normalize_or(Vec2::X);
                    let towards_road = Vec2::new(-t.y, t.x) * -side;
                    m.sidewalks.wall(a, b, 0.0, CURB_Z, towards_road);
                }
            }
        }
    }
    // Zebra crossings: white bars across each crossing, 0.5 m wide and
    // 1 m apart, over the carriageway between the two sidewalks.
    for e in &net.sidewalks.edges {
        let crate::sidewalk::WalkKind::Crossing { road, .. } = e.kind else {
            continue;
        };
        let (a, b) = (e.path.points[0], *e.path.points.last().expect("two points"));
        let d = (b - a).normalize_or_zero();
        let n = Vec2::new(-d.y, d.x);
        let half = net.roads[road].def.sidewalk_m / 2.0;
        let span = a.distance(b) - 2.0 * half;
        let bars = (span / 1.0).floor() as i32;
        for k in 0..bars {
            let p = a + d * (half + 0.5 + k as f32);
            let left = [p - d * 0.25 + n * 1.5, p + d * 0.25 + n * 1.5];
            let right = [p - d * 0.25 - n * 1.5, p + d * 0.25 - n * 1.5];
            m.markings.strip(&left, &right, MARKING_Z);
        }
    }
    // Junction areas: a fan over the road ends' corner points.
    for j in &net.junctions {
        if j.corners.len() < 3 {
            continue;
        }
        let centre = j.corners.iter().copied().sum::<Vec2>() / j.corners.len() as f32;
        let c = m.asphalt.vertex(centre.extend(ASPHALT_Z), Vec3::Z);
        let first = m.asphalt.positions.len() as u32;
        for p in &j.corners {
            m.asphalt.vertex(p.extend(ASPHALT_Z), Vec3::Z);
        }
        let n = j.corners.len() as u32;
        for i in 0..n {
            m.asphalt
                .indices
                .extend_from_slice(&[c, first + i, first + (i + 1) % n]);
        }
    }
    m
}

/// A road whose width changes at a lane taper: edges, lane lines and
/// sidewalks follow the lanes as they narrow or open.
fn tapered_road(m: &mut RoadMeshes, road: &RoadGeometry, c: &Polyline) {
    let left = c.offset_with(|s| road.edges(s).0);
    let right = c.offset_with(|s| road.edges(s).1);
    m.asphalt.strip(&left.points, &right.points, ASPHALT_Z);

    let d = &road.def;
    let w = d.lane_width_m;
    let len = c.length();
    if d.lanes_forward > 0 && d.lanes_backward > 0 {
        // The centre line sits between the innermost lanes of each direction.
        let divider =
            |s: f32| (road.lane_offset(true, 0, s).0 + road.lane_offset(false, 0, s).0) / 2.0;
        if d.total_lanes() >= 4 {
            paint(&mut m.markings, c.offset_with(|s| divider(s) + 0.12), false);
            paint(&mut m.markings, c.offset_with(|s| divider(s) - 0.12), false);
        } else {
            paint(&mut m.markings, c.offset_with(divider), true);
        }
    }
    // Lane lines between neighbours of one direction, only where both lanes
    // are at least half open.
    for (forward, n) in [(true, d.lanes_forward), (false, d.lanes_backward)] {
        for k in 1..n as usize {
            let open = |s: f32| {
                road.lane_offset(forward, k, s).1 >= 0.5 * w
                    && road.lane_offset(forward, k - 1, s).1 >= 0.5 * w
            };
            let (mut from, mut to) = (None, 0.0);
            for &s in &c.lengths {
                if open(s) {
                    from.get_or_insert(s);
                    to = s;
                }
            }
            let Some(from) = from else { continue };
            let between = |s: f32| {
                let (a, wa) = road.lane_offset(forward, k - 1, s);
                let (b, wb) = road.lane_offset(forward, k, s);
                // The shared edge: halfway between the facing sides.
                let (ea, eb) = if b > a {
                    (a + wa / 2.0, b - wb / 2.0)
                } else {
                    (a - wa / 2.0, b + wb / 2.0)
                };
                (ea + eb) / 2.0
            };
            let part = c.slice(from, to.min(len));
            if part.length() > 1.0 {
                // `part` starts at `from`, so shift its arc lengths back.
                paint(
                    &mut m.markings,
                    part.offset_with(|s| between(s + from)),
                    true,
                );
            }
        }
    }
    paint(
        &mut m.markings,
        c.offset_with(|s| road.edges(s).0 - 0.3),
        false,
    );
    paint(
        &mut m.markings,
        c.offset_with(|s| road.edges(s).1 + 0.3),
        false,
    );

    if d.sidewalk_m > 0.0 {
        for side in [1.0f32, -1.0] {
            let edge = |s: f32| {
                let (l, r) = road.edges(s);
                if side > 0.0 { l } else { r }
            };
            let inner = c.offset_with(edge);
            let outer = c.offset_with(|s| edge(s) + side * d.sidewalk_m);
            if side > 0.0 {
                m.sidewalks.strip(&outer.points, &inner.points, CURB_Z);
            } else {
                m.sidewalks.strip(&inner.points, &outer.points, CURB_Z);
            }
            for i in 1..inner.points.len() {
                let (a, b) = (inner.points[i - 1], inner.points[i]);
                let t = (b - a).normalize_or(Vec2::X);
                let towards_road = Vec2::new(-t.y, t.x) * -side;
                m.sidewalks.wall(a, b, 0.0, CURB_Z, towards_road);
            }
        }
    }
}

/// A square piece of the road meshes, for streaming: the renderer keeps
/// only tiles near the camera on the GPU. Positions are relative to
/// `centre`, so big maps stay precise in 32-bit floats.
#[derive(Clone, Debug, Default)]
pub struct RoadTile {
    /// Tile column and row: `floor(x / size)`, `floor(y / size)`.
    pub key: (i32, i32),
    /// Tile centre, m (world).
    pub centre: [f32; 2],
    /// Asphalt, markings and sidewalks, as in [`RoadMeshes`].
    pub layers: [Mesh; 3],
    /// Bounds of the tile's vertices, relative to `centre`.
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl RoadTile {
    pub fn triangle_count(&self) -> usize {
        self.layers.iter().map(Mesh::triangle_count).sum()
    }
}

/// Default tile edge, m: big enough to keep draw calls low on phones.
pub const TILE_M: f32 = 400.0;

/// Cut road meshes into `tile_m` squares. A triangle goes to the tile that
/// holds its centre, so nothing is drawn twice. Tiles are in key order.
pub fn split_tiles(meshes: &RoadMeshes, tile_m: f32) -> Vec<RoadTile> {
    use std::collections::{BTreeMap, HashMap};
    // Each tile with, per layer, a map from source vertex to tile vertex.
    type Remap = [HashMap<u32, u32>; 3];
    let mut tiles: BTreeMap<(i32, i32), (RoadTile, Remap)> = BTreeMap::new();
    let layers = [&meshes.asphalt, &meshes.markings, &meshes.sidewalks];
    for (k, mesh) in layers.iter().enumerate() {
        for t in mesh.indices.chunks(3) {
            let p = |i: u32| Vec3::from(mesh.positions[i as usize]);
            let c = (p(t[0]) + p(t[1]) + p(t[2])) / 3.0;
            let key = ((c.x / tile_m).floor() as i32, (c.y / tile_m).floor() as i32);
            let (tile, remap) = tiles.entry(key).or_insert_with(|| {
                let centre = [(key.0 as f32 + 0.5) * tile_m, (key.1 as f32 + 0.5) * tile_m];
                (
                    RoadTile {
                        key,
                        centre,
                        min: [f32::MAX; 3],
                        max: [f32::MIN; 3],
                        ..Default::default()
                    },
                    Default::default(),
                )
            });
            for &i in t {
                let out = &mut tile.layers[k];
                let j = *remap[k].entry(i).or_insert_with(|| {
                    let q = mesh.positions[i as usize];
                    let local = [q[0] - tile.centre[0], q[1] - tile.centre[1], q[2]];
                    for (a, v) in local.iter().enumerate() {
                        tile.min[a] = tile.min[a].min(*v);
                        tile.max[a] = tile.max[a].max(*v);
                    }
                    out.positions.push(local);
                    out.normals.push(mesh.normals[i as usize]);
                    out.positions.len() as u32 - 1
                });
                out.indices.push(j);
            }
        }
    }
    tiles.into_values().map(|(t, _)| t).collect()
}

/// [`build_meshes`] cut into tiles of `tile_m`.
pub fn build_tiles(net: &RoadNetwork, tile_m: f32) -> Vec<RoadTile> {
    split_tiles(&build_meshes(net), tile_m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::*;
    use crate::network::RoadNetwork;

    #[test]
    fn refining_splits_long_edges_without_changing_the_surface() {
        let mut m = Mesh::default();
        let l = [Vec2::new(0.0, 0.0), Vec2::new(0.0, 10.0)];
        let r = [Vec2::new(7.0, 0.0), Vec2::new(7.0, 10.0)];
        m.strip(&l, &r, 0.0);
        let area = |m: &Mesh| -> f32 {
            m.indices
                .chunks(3)
                .map(|t| {
                    let p = |i: u32| Vec3::from(m.positions[i as usize]);
                    (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).z * 0.5
                })
                .sum()
        };
        let before = area(&m);
        m.refine(2.0);
        assert!((area(&m) - before).abs() < 1e-3, "{} vs {before}", area(&m));
        for t in m.indices.chunks(3) {
            for k in 0..3 {
                let (a, b) = (
                    m.positions[t[k] as usize],
                    m.positions[t[(k + 1) % 3] as usize],
                );
                assert!(Vec2::new(a[0] - b[0], a[1] - b[1]).length() <= 2.0 + 1e-4);
            }
            // Still facing up (counter-clockwise from above).
            let p = |i: u32| Vec3::from(m.positions[i as usize]);
            assert!((p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).z > 0.0);
        }
    }

    #[test]
    fn meshes_cover_roads_and_face_up() {
        let net = RoadNetwork::build(&RoadNetworkDef {
            stops: Vec::new(),
            name: "t".into(),
            drive_side: DriveSide::Right,
            nodes: vec![
                NodeDef {
                    id: "a".into(),
                    position: (0.0, 0.0),
                    control: Default::default(),
                },
                NodeDef {
                    id: "b".into(),
                    position: (60.0, 0.0),
                    control: Default::default(),
                },
            ],
            roads: vec![RoadDef {
                id: "r".into(),
                name: None,
                from: "a".into(),
                to: "b".into(),
                shape: RoadShape::Straight,
                lanes_forward: 1,
                lanes_backward: 1,
                lane_width_m: 3.5,
                speed_limit_kmh: 50.0,
                sidewalk_m: 2.0,
            }],
        });
        let m = build_meshes(&net);
        assert!(m.asphalt.triangle_count() > 50);
        assert!(m.markings.triangle_count() > 10, "dashes and edge lines");
        assert!(m.sidewalks.triangle_count() > 50);
        // Every asphalt triangle must face up (counter-clockwise from above).
        for t in m.asphalt.indices.chunks(3) {
            let p = |i: u32| Vec3::from(m.asphalt.positions[i as usize]);
            let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            assert!(n.z >= -1e-4, "downward-facing asphalt triangle {t:?}");
        }
    }

    #[test]
    fn tiles_keep_every_triangle() {
        let def: RoadNetworkDef = wp_data::parse_ron(
            "town.road.ron",
            include_str!("../../../examples/sandbox/roads/town.road.ron"),
        )
        .unwrap();
        let net = RoadNetwork::build(&def);
        let whole = build_meshes(&net);
        let tiles = split_tiles(&whole, 100.0);
        assert!(tiles.len() > 4);
        let total: usize = tiles.iter().map(RoadTile::triangle_count).sum();
        let want = whole.asphalt.triangle_count()
            + whole.markings.triangle_count()
            + whole.sidewalks.triangle_count();
        assert_eq!(total, want);
        for t in &tiles {
            // Vertices stay near their tile (triangles may overhang a little).
            assert!(t.min[0] > -100.0 && t.max[0] < 100.0, "{:?}", t.key);
            assert!(
                t.layers
                    .iter()
                    .all(|l| l.indices.iter().all(|&i| (i as usize) < l.positions.len()))
            );
        }
    }
}
