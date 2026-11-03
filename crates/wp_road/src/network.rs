//! The built road network: trimmed road geometry, lanes, junction connectors
//! and routing. Built once from a [`RoadNetworkDef`]; read-only afterwards.

use crate::def::{DriveSide, RoadDef, RoadNetworkDef};
use crate::geometry::{Polyline, centreline, connector};
use glam::Vec2;
use std::collections::{BTreeMap, BinaryHeap};

pub type LaneId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    /// Lane `index` of road `road` (0 = next to the centreline); `forward`
    /// means it runs from the road's `from` node to its `to` node.
    Road {
        road: usize,
        forward: bool,
        index: u8,
    },
    /// A path across junction `node`.
    Connector { node: usize, turn: Turn },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Straight,
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub struct Lane {
    /// Stable name: `road:f0` / `road:b1` for road lanes, `node>k` for connectors.
    pub name: String,
    pub kind: LaneKind,
    /// Centre of the lane in its direction of travel.
    pub path: Polyline,
    pub width_m: f32,
    pub speed_limit_kmh: f32,
    /// Lanes a vehicle can continue into from this lane's end.
    pub next: Vec<LaneId>,
}

/// A road's drawable geometry after trimming at junctions.
#[derive(Clone, Debug)]
pub struct RoadGeometry {
    pub def: RoadDef,
    /// Centreline from `from` to `to`, trimmed at both junctions.
    pub centre: Polyline,
    /// Carriageway width left and right of the centreline (in its direction).
    pub left_m: f32,
    pub right_m: f32,
}

/// A junction (any node joined by two or more roads).
#[derive(Clone, Debug)]
pub struct Junction {
    pub node: usize,
    pub position: Vec2,
    /// Carriageway edge points of every road end, sorted around the junction.
    pub corners: Vec<Vec2>,
}

#[derive(Clone, Debug)]
pub struct RoadNetwork {
    pub name: String,
    pub drive_side: DriveSide,
    pub roads: Vec<RoadGeometry>,
    pub lanes: Vec<Lane>,
    pub junctions: Vec<Junction>,
}

impl RoadNetwork {
    /// Build from a definition. Roads that refer to missing nodes are skipped
    /// (`wp check` reports them).
    pub fn build(def: &RoadNetworkDef) -> RoadNetwork {
        let pos = |id: &str| def.node(id).map(|n| Vec2::new(n.position.0, n.position.1));
        let node_index: BTreeMap<&str, usize> = def
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();
        // Full (untrimmed) centrelines.
        let mut full = Vec::new();
        for r in &def.roads {
            if let (Some(a), Some(b)) = (pos(&r.from), pos(&r.to))
                && r.from != r.to
            {
                full.push((r.clone(), centreline(&r.shape, a, b)));
            }
        }
        // Roads at each node, with the direction leaving the node.
        let mut at_node: BTreeMap<usize, Vec<(usize, bool, Vec2)>> = BTreeMap::new();
        for (ri, (r, line)) in full.iter().enumerate() {
            at_node
                .entry(node_index[r.from.as_str()])
                .or_default()
                .push((ri, true, line.start_direction()));
            at_node.entry(node_index[r.to.as_str()]).or_default().push((
                ri,
                false,
                -line.end_direction(),
            ));
        }
        // Trim each road end so junction areas do not overlap.
        let half_width = |r: &RoadDef| r.carriageway_width() / 2.0 + r.sidewalk_m;
        let mut trim_start = vec![0.0f32; full.len()];
        let mut trim_end = vec![0.0f32; full.len()];
        for ends in at_node.values() {
            if ends.len() < 2 {
                continue;
            }
            let radius = ends
                .iter()
                .map(|(ri, _, _)| half_width(&full[*ri].0))
                .fold(0.0, f32::max)
                + 1.0;
            let trim = if ends.len() == 2 {
                // A bend: trim in proportion to how sharply the roads meet.
                let straightness = (-ends[0].2.dot(ends[1].2)).clamp(-1.0, 1.0);
                radius * (1.0 - straightness).min(1.0) + 0.2
            } else {
                radius
            };
            for (ri, starts_here, _) in ends {
                let len = full[*ri].1.length();
                let t = trim.min(len * 0.45);
                if *starts_here {
                    trim_start[*ri] = t;
                } else {
                    trim_end[*ri] = t;
                }
            }
        }
        let side = match def.drive_side {
            DriveSide::Right => 1.0,
            DriveSide::Left => -1.0,
        };
        let mut roads = Vec::new();
        let mut lanes = Vec::new();
        for (ri, (r, line)) in full.iter().enumerate() {
            let centre = line.trim(trim_start[ri], trim_end[ri]);
            let w = r.lane_width_m;
            // Right-hand traffic: forward lanes on the right (negative offsets).
            let (right_m, left_m) = if side > 0.0 {
                (r.lanes_forward as f32 * w, r.lanes_backward as f32 * w)
            } else {
                (r.lanes_backward as f32 * w, r.lanes_forward as f32 * w)
            };
            for i in 0..r.lanes_forward {
                lanes.push(Lane {
                    name: format!("{}:f{i}", r.id),
                    kind: LaneKind::Road {
                        road: ri,
                        forward: true,
                        index: i,
                    },
                    path: centre.offset(-side * (i as f32 + 0.5) * w),
                    width_m: w,
                    speed_limit_kmh: r.speed_limit_kmh,
                    next: Vec::new(),
                });
            }
            for i in 0..r.lanes_backward {
                lanes.push(Lane {
                    name: format!("{}:b{i}", r.id),
                    kind: LaneKind::Road {
                        road: ri,
                        forward: false,
                        index: i,
                    },
                    path: centre.offset(side * (i as f32 + 0.5) * w).reversed(),
                    width_m: w,
                    speed_limit_kmh: r.speed_limit_kmh,
                    next: Vec::new(),
                });
            }
            roads.push(RoadGeometry {
                def: r.clone(),
                centre,
                left_m,
                right_m,
            });
        }

        // Junction connectors.
        let mut junctions = Vec::new();
        for (&node, ends) in &at_node {
            let p = pos(&def.nodes[node].id).unwrap_or_default();
            if ends.len() >= 2 {
                let mut corners: Vec<Vec2> = Vec::new();
                for (ri, starts_here, _) in ends {
                    let g = &roads[*ri];
                    let (pt, dir) = if *starts_here {
                        g.centre.sample(0.0)
                    } else {
                        g.centre.sample(g.centre.length())
                    };
                    let left = Vec2::new(-dir.y, dir.x);
                    corners.push(pt + left * g.left_m);
                    corners.push(pt - left * g.right_m);
                }
                corners.sort_by(|a, b| (*a - p).to_angle().total_cmp(&(*b - p).to_angle()));
                junctions.push(Junction {
                    node,
                    position: p,
                    corners,
                });
            }
            // Lanes arriving at / leaving this node.
            let arriving: Vec<LaneId> = lane_ids_at(&lanes, &roads, ends, false);
            let leaving: Vec<LaneId> = lane_ids_at(&lanes, &roads, ends, true);
            let mut k = 0;
            for &inn in &arriving {
                for &out in &leaving {
                    let (
                        LaneKind::Road {
                            road: r_in,
                            index: i_in,
                            ..
                        },
                        LaneKind::Road {
                            road: r_out,
                            index: i_out,
                            ..
                        },
                    ) = (lanes[inn].kind, lanes[out].kind)
                    else {
                        continue;
                    };
                    if r_in == r_out {
                        continue; // no U-turns
                    }
                    let t_in = lanes[inn].path.end_direction();
                    let t_out = lanes[out].path.start_direction();
                    let delta = t_in.angle_to(t_out);
                    let turn = if delta.abs() < 0.5 {
                        Turn::Straight
                    } else if delta.abs() > 2.6 {
                        continue;
                    } else if delta > 0.0 {
                        Turn::Left
                    } else {
                        Turn::Right
                    };
                    let n_in = lane_count(&roads[r_in].def, lanes[inn].kind);
                    let n_out = lane_count(&roads[r_out].def, lanes[out].kind);
                    // Turns across traffic use the inside lane; turns away from
                    // it the outside lane (mirrored for left-hand traffic).
                    let (across, away) = if side > 0.0 {
                        (Turn::Left, Turn::Right)
                    } else {
                        (Turn::Right, Turn::Left)
                    };
                    // Where only two roads meet the road simply continues
                    // (a bend, not a junction): every lane flows on.
                    let continuation = ends.len() == 2;
                    let allowed = match turn {
                        _ if continuation => i_out == i_in.min(n_out - 1),
                        Turn::Straight => i_out == i_in.min(n_out - 1),
                        t if t == across => i_in == 0 && i_out == 0,
                        t if t == away => i_in == n_in - 1 && i_out == n_out - 1,
                        _ => false,
                    };
                    if !allowed {
                        continue;
                    }
                    let a = lanes[inn].path.points.last().copied().unwrap_or(p);
                    let b = lanes[out].path.points.first().copied().unwrap_or(p);
                    let id = lanes.len();
                    let speed = lanes[inn].speed_limit_kmh.min(if turn == Turn::Straight {
                        50.0
                    } else {
                        25.0
                    });
                    lanes.push(Lane {
                        name: format!("{}>{k}", def.nodes[node].id),
                        kind: LaneKind::Connector { node, turn },
                        path: connector(a, t_in, b, t_out, 10),
                        width_m: lanes[inn].width_m,
                        speed_limit_kmh: speed,
                        next: vec![out],
                    });
                    lanes[inn].next.push(id);
                    k += 1;
                }
            }
        }
        RoadNetwork {
            name: def.name.clone(),
            drive_side: def.drive_side,
            roads,
            lanes,
            junctions,
        }
    }

    pub fn lane(&self, name: &str) -> Option<LaneId> {
        self.lanes.iter().position(|l| l.name == name)
    }

    /// The road lane nearest to `p` within its width: (lane, arc length, lateral offset).
    pub fn lane_at(&self, p: Vec2) -> Option<(LaneId, f32, f32)> {
        let mut best: Option<(LaneId, f32, f32, f32)> = None;
        for (i, lane) in self.lanes.iter().enumerate() {
            if !matches!(lane.kind, LaneKind::Road { .. }) {
                continue;
            }
            let (s, lat, dist) = lane.path.project(p);
            if dist <= lane.width_m / 2.0 + 0.5 && best.is_none_or(|b| dist < b.3) {
                best = Some((i, s, lat, dist));
            }
        }
        best.map(|(i, s, lat, _)| (i, s, lat))
    }

    /// Fastest lane sequence from `from` to any lane of road `to_road`
    /// (A* on travel time; straight-line distance at 50 km/h as the heuristic).
    pub fn route(&self, from: LaneId, to_road: &str) -> Option<Vec<LaneId>> {
        let goal_road = self.roads.iter().position(|r| r.def.id == to_road)?;
        let is_goal = |l: LaneId| matches!(self.lanes[l].kind, LaneKind::Road { road, .. } if road == goal_road);
        let goal_point = self.roads[goal_road]
            .centre
            .sample(self.roads[goal_road].centre.length() / 2.0)
            .0;
        let cost = |l: LaneId| {
            self.lanes[l].path.length() / (self.lanes[l].speed_limit_kmh / 3.6).max(1.0)
        };
        let h = |l: LaneId| {
            self.lanes[l]
                .path
                .points
                .last()
                .map(|p| p.distance(goal_point))
                .unwrap_or(0.0)
                / (50.0 / 3.6)
        };
        #[derive(PartialEq)]
        struct Item(f32, LaneId);
        impl Eq for Item {}
        impl Ord for Item {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
            }
        }
        impl PartialOrd for Item {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        let mut best = vec![f32::INFINITY; self.lanes.len()];
        let mut came = vec![usize::MAX; self.lanes.len()];
        let mut open = BinaryHeap::new();
        best[from] = 0.0;
        open.push(Item(h(from), from));
        while let Some(Item(_, l)) = open.pop() {
            if is_goal(l) && l != from {
                let mut path = vec![l];
                while path.last() != Some(&from) {
                    path.push(came[*path.last().unwrap()]);
                }
                path.reverse();
                return Some(path);
            }
            for &n in &self.lanes[l].next {
                let g = best[l] + cost(l);
                if g < best[n] {
                    best[n] = g;
                    came[n] = l;
                    open.push(Item(g + h(n), n));
                }
            }
        }
        None
    }

    /// The centre path of a lane sequence, joined end to end.
    pub fn route_path(&self, route: &[LaneId]) -> Polyline {
        let parts: Vec<&Polyline> = route.iter().map(|&l| &self.lanes[l].path).collect();
        Polyline::concat(&parts)
    }
}

fn lane_count(def: &RoadDef, kind: LaneKind) -> u8 {
    match kind {
        LaneKind::Road { forward: true, .. } => def.lanes_forward,
        LaneKind::Road { forward: false, .. } => def.lanes_backward,
        LaneKind::Connector { .. } => 1,
    }
}

/// Road lanes that start (`leaving`) or end (`!leaving`) at a node, given the
/// node's road ends.
fn lane_ids_at(
    lanes: &[Lane],
    roads: &[RoadGeometry],
    ends: &[(usize, bool, Vec2)],
    leaving: bool,
) -> Vec<LaneId> {
    let _ = roads;
    let mut out = Vec::new();
    for (ri, starts_here, _) in ends {
        for (id, lane) in lanes.iter().enumerate() {
            if let LaneKind::Road { road, forward, .. } = lane.kind
                && road == *ri
            {
                // A forward lane starts at the road's `from` node.
                let starts_at_node = forward == *starts_here;
                if starts_at_node == leaving {
                    out.push(id);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{NodeDef, RoadShape};

    fn road(id: &str, from: &str, to: &str) -> RoadDef {
        RoadDef {
            id: id.into(),
            name: None,
            from: from.into(),
            to: to.into(),
            shape: RoadShape::Straight,
            lanes_forward: 1,
            lanes_backward: 1,
            lane_width_m: 3.5,
            speed_limit_kmh: 50.0,
            sidewalk_m: 0.0,
        }
    }

    /// A crossroads: centre "c" with arms to n, e, s, w.
    fn crossroads() -> RoadNetworkDef {
        let node = |id: &str, x: f32, y: f32| NodeDef {
            id: id.into(),
            position: (x, y),
        };
        RoadNetworkDef {
            name: "cross".into(),
            drive_side: DriveSide::Right,
            nodes: vec![
                node("c", 0.0, 0.0),
                node("n", 0.0, 100.0),
                node("e", 100.0, 0.0),
                node("s", 0.0, -100.0),
                node("w", -100.0, 0.0),
            ],
            roads: vec![
                road("north", "c", "n"),
                road("east", "c", "e"),
                road("south", "s", "c"),
                road("west", "w", "c"),
            ],
        }
    }

    #[test]
    fn right_hand_lanes_sit_on_the_right() {
        let net = RoadNetwork::build(&crossroads());
        let east = &net.lanes[net.lane("east:f0").unwrap()];
        // East road runs +x; its forward lane must be south of the centreline.
        assert!(east.path.points[0].y < 0.0, "{:?}", east.path.points[0]);
        let back = &net.lanes[net.lane("east:b0").unwrap()];
        assert!(back.path.points[0].y > 0.0 && back.path.start_direction().x < 0.0);
    }

    #[test]
    fn crossroads_has_straight_left_and_right_connectors() {
        let net = RoadNetwork::build(&crossroads());
        let from_south = net.lane("south:f0").unwrap(); // heading north into the junction
        let turns: Vec<Turn> = net.lanes[from_south]
            .next
            .iter()
            .map(|&c| match net.lanes[c].kind {
                LaneKind::Connector { turn, .. } => turn,
                _ => panic!("road lanes connect through connectors"),
            })
            .collect();
        assert_eq!(turns.len(), 3, "{turns:?}");
        for t in [Turn::Straight, Turn::Left, Turn::Right] {
            assert!(turns.contains(&t), "missing {t:?}");
        }
    }

    #[test]
    fn routes_through_the_junction() {
        let net = RoadNetwork::build(&crossroads());
        let start = net.lane("south:f0").unwrap();
        let route = net.route(start, "east").expect("a route east");
        let names: Vec<&str> = route.iter().map(|&l| net.lanes[l].name.as_str()).collect();
        assert_eq!(names.first(), Some(&"south:f0"));
        assert_eq!(names.last(), Some(&"east:f0"));
        assert_eq!(names.len(), 3, "{names:?}");
        let path = net.route_path(&route);
        assert!(
            path.length() > 180.0 && path.length() < 220.0,
            "len {}",
            path.length()
        );
    }

    #[test]
    fn a_bend_lets_every_lane_continue() {
        let node = |id: &str, x: f32, y: f32| NodeDef {
            id: id.into(),
            position: (x, y),
        };
        let mut wide = road("a", "p", "q");
        wide.lanes_forward = 2;
        let def = RoadNetworkDef {
            name: "bend".into(),
            drive_side: DriveSide::Right,
            nodes: vec![
                node("p", 0.0, 0.0),
                node("q", 100.0, 0.0),
                node("r", 160.0, 80.0),
            ],
            roads: vec![wide, road("b", "q", "r")],
        };
        let net = RoadNetwork::build(&def);
        for lane in ["a:f0", "a:f1"] {
            let id = net.lane(lane).unwrap();
            assert!(
                net.route(id, "b").is_some(),
                "{lane} cannot continue round the bend"
            );
        }
    }

    #[test]
    fn lane_lookup_finds_the_lane_under_a_point() {
        let net = RoadNetwork::build(&crossroads());
        let (lane, _, _) = net.lane_at(Vec2::new(50.0, -1.75)).unwrap();
        assert_eq!(net.lanes[lane].name, "east:f0");
        assert!(net.lane_at(Vec2::new(50.0, 30.0)).is_none());
    }
}
