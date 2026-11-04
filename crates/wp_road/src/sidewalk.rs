//! The sidewalk graph, built from the road network: a walking path down the
//! middle of every sidewalk, zebra crossings across each road where it meets
//! a junction (or ends), and links round junction corners between the
//! sidewalks of neighbouring roads. Pedestrians route on it.

use crate::geometry::Polyline;
use crate::network::RoadGeometry;
use glam::Vec2;
use std::collections::{BTreeMap, BinaryHeap};

/// A place where walks join (a sidewalk end).
pub type WalkNode = usize;

#[derive(Clone, Debug, PartialEq)]
pub enum WalkKind {
    /// Along a sidewalk.
    Sidewalk,
    /// A zebra crossing over `road`, at the junction at node index `node`.
    Crossing { road: usize, node: usize },
    /// Round a junction corner, between two roads' sidewalks.
    Corner,
}

#[derive(Clone, Debug)]
pub struct WalkEdge {
    pub from: WalkNode,
    pub to: WalkNode,
    pub path: Polyline,
    pub kind: WalkKind,
}

#[derive(Clone, Debug, Default)]
pub struct SidewalkGraph {
    pub nodes: Vec<Vec2>,
    pub edges: Vec<WalkEdge>,
    /// Edges leaving each node (walks go both ways: edge index, forwards?).
    pub adjacent: Vec<Vec<(usize, bool)>>,
}

impl SidewalkGraph {
    /// Build from trimmed road geometry. `arms_at` lists, for each node index
    /// where roads end, the roads ending there and whether each starts there.
    pub fn build(roads: &[RoadGeometry], arms_at: &BTreeMap<usize, Vec<(usize, bool)>>) -> Self {
        let mut g = SidewalkGraph::default();
        // Sidewalk ends at each node: (road, left side?, point).
        let mut ends: BTreeMap<usize, Vec<(usize, bool, WalkNode)>> = BTreeMap::new();
        for (ri, road) in roads.iter().enumerate() {
            let w = road.def.sidewalk_m;
            if w <= 0.0 || road.centre.length() < 2.0 {
                continue;
            }
            let c = road.centre.resample(2.0);
            for left in [true, false] {
                let off = |s: f32| {
                    let (l, r) = road.edges(s);
                    if left { l + w / 2.0 } else { r - w / 2.0 }
                };
                let path = c.offset_with(off);
                let (a, b) = (
                    g.node(path.points[0]),
                    g.node(*path.points.last().expect("not empty")),
                );
                g.edge(a, b, path, WalkKind::Sidewalk);
                ends.entry(road.from_node).or_default().push((ri, left, a));
                ends.entry(road.to_node).or_default().push((ri, left, b));
            }
        }
        for (&node, list) in &ends {
            let Some(arms) = arms_at.get(&node) else {
                continue;
            };
            // A crossing over each road with sidewalks on both sides, where
            // three or more roads meet or the road ends.
            if arms.len() != 2 {
                let mut roads_here: Vec<usize> = list.iter().map(|e| e.0).collect();
                roads_here.dedup();
                for ri in roads_here {
                    let sides: Vec<WalkNode> =
                        list.iter().filter(|e| e.0 == ri).map(|e| e.2).collect();
                    if let [a, b] = sides[..] {
                        let path = Polyline::new(vec![g.nodes[a], g.nodes[b]]);
                        g.edge(a, b, path, WalkKind::Crossing { road: ri, node });
                    }
                }
            }
            // Corners: going round the node, neighbouring ends of different roads.
            let centre = list.iter().map(|e| g.nodes[e.2]).sum::<Vec2>() / list.len() as f32;
            let mut around: Vec<(usize, WalkNode)> = list.iter().map(|e| (e.0, e.2)).collect();
            around.sort_by(|a, b| {
                (g.nodes[a.1] - centre)
                    .to_angle()
                    .total_cmp(&(g.nodes[b.1] - centre).to_angle())
            });
            if around.len() >= 2 {
                for k in 0..around.len() {
                    let (a, b) = (around[k], around[(k + 1) % around.len()]);
                    if a.0 != b.0 && !(around.len() == 2 && k == 1) {
                        let path = Polyline::new(vec![g.nodes[a.1], g.nodes[b.1]]);
                        if path.length() < 40.0 {
                            g.edge(a.1, b.1, path, WalkKind::Corner);
                        }
                    }
                }
            }
        }
        g
    }

    fn node(&mut self, p: Vec2) -> WalkNode {
        self.nodes.push(p);
        self.adjacent.push(Vec::new());
        self.nodes.len() - 1
    }

    fn edge(&mut self, from: WalkNode, to: WalkNode, path: Polyline, kind: WalkKind) {
        let id = self.edges.len();
        self.edges.push(WalkEdge {
            from,
            to,
            path,
            kind,
        });
        self.adjacent[from].push((id, true));
        self.adjacent[to].push((id, false));
    }

    /// Shortest walk from node `from` to node `to`: the edges and the
    /// direction each is walked (true = from its `from` to its `to`).
    pub fn route(&self, from: WalkNode, to: WalkNode) -> Option<Vec<(usize, bool)>> {
        #[derive(PartialEq)]
        struct Item(f32, WalkNode);
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
        let goal = self.nodes[to];
        let mut best = vec![f32::INFINITY; self.nodes.len()];
        let mut came: Vec<Option<(usize, bool)>> = vec![None; self.nodes.len()];
        let mut open = BinaryHeap::new();
        best[from] = 0.0;
        open.push(Item(self.nodes[from].distance(goal), from));
        while let Some(Item(_, n)) = open.pop() {
            if n == to {
                let mut walk = Vec::new();
                let mut at = to;
                while let Some((e, fwd)) = came[at] {
                    walk.push((e, fwd));
                    at = if fwd {
                        self.edges[e].from
                    } else {
                        self.edges[e].to
                    };
                }
                walk.reverse();
                return Some(walk);
            }
            for &(e, fwd) in &self.adjacent[n] {
                let edge = &self.edges[e];
                let next = if fwd { edge.to } else { edge.from };
                let g = best[n] + edge.path.length();
                if g < best[next] {
                    best[next] = g;
                    came[next] = Some((e, fwd));
                    open.push(Item(g + self.nodes[next].distance(goal), next));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::def::*;
    use crate::network::RoadNetwork;

    /// A crossroads with 2 m sidewalks on every arm.
    fn crossroads() -> RoadNetwork {
        let node = |id: &str, x: f32, y: f32| NodeDef {
            id: id.into(),
            position: (x, y),
            control: Default::default(),
        };
        let road = |id: &str, from: &str, to: &str| RoadDef {
            id: id.into(),
            name: None,
            from: from.into(),
            to: to.into(),
            shape: RoadShape::Straight,
            lanes_forward: 1,
            lanes_backward: 1,
            lane_width_m: 3.5,
            speed_limit_kmh: 50.0,
            sidewalk_m: 2.0,
        };
        RoadNetwork::build(&RoadNetworkDef {
            name: "x".into(),
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
        })
    }

    #[test]
    fn crossroads_have_crossings_corners_and_routes() {
        let net = crossroads();
        let g = &net.sidewalks;
        let count = |k: fn(&super::WalkKind) -> bool| g.edges.iter().filter(|e| k(&e.kind)).count();
        assert_eq!(
            count(|k| matches!(k, super::WalkKind::Sidewalk)),
            8,
            "two per road"
        );
        // Four crossings at the centre, one at each dead end.
        assert_eq!(count(|k| matches!(k, super::WalkKind::Crossing { .. })), 8);
        assert_eq!(
            count(|k| matches!(k, super::WalkKind::Corner)),
            4,
            "round each corner"
        );
        // Sidewalk lines run 1 m outside the 3.5 m lanes: 4.5 m from the centreline.
        let first = g.edges[0].path.points[0];
        assert!(
            (first.y.abs() - 4.5).abs() < 0.05 || (first.x.abs() - 4.5).abs() < 0.05,
            "{first}"
        );
        // Every node reaches every other.
        for to in 0..g.nodes.len() {
            assert!(g.route(0, to).is_some(), "node {to} unreachable");
        }
    }
}
