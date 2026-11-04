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
}

/// The three road layers, each drawn with its own material.
#[derive(Clone, Debug, Default)]
pub struct RoadMeshes {
    pub asphalt: Mesh,
    pub markings: Mesh,
    pub sidewalks: Mesh,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::*;
    use crate::network::RoadNetwork;

    #[test]
    fn meshes_cover_roads_and_face_up() {
        let net = RoadNetwork::build(&RoadNetworkDef {
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
}
