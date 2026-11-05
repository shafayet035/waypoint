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
    /// For a junction path: the road it comes from.
    pub approach: Option<usize>,
    /// Neighbouring lanes in the same direction a vehicle can change into
    /// (empty on roads shorter than [`MIN_LANE_CHANGE_M`]).
    pub changes: Vec<LaneId>,
}

/// Radius of the kerb at junction corners, m: turning paths are at least this
/// plus half a lane, wider than a car's or bus's tightest turn.
pub const KERB_RADIUS_M: f32 = 6.0;

/// Junction paths tighter than this are not offered, m (a hatchback turns in
/// about 5.5 m).
const MIN_TURN_RADIUS_M: f32 = 5.0;
/// Lanes bend through a node where a road continues at this radius, m.
const BEND_RADIUS_M: f32 = 15.0;

/// Shortest lane on which a lane change is planned, m.
pub const MIN_LANE_CHANGE_M: f32 = 30.0;
/// Routing penalty for one lane change, s (it is slower and riskier than staying).
const LANE_CHANGE_PENALTY_S: f32 = 4.0;

/// A road's drawable geometry after trimming at junctions.
#[derive(Clone, Debug)]
pub struct RoadGeometry {
    pub def: RoadDef,
    /// Centreline from `from` to `to`, trimmed at both junctions.
    pub centre: Polyline,
    /// Carriageway width left and right of the centreline (in its direction),
    /// away from any taper.
    pub left_m: f32,
    pub right_m: f32,
    /// Where each lane's centre sits, m left of the centreline, away from tapers.
    pub forward_offsets: Vec<f32>,
    pub backward_offsets: Vec<f32>,
    /// Tapers at the start and end, where the lane count changes into the next road.
    pub tapers: [Option<Taper>; 2],
    /// Indices of the nodes it runs from and to.
    pub from_node: usize,
    pub to_node: usize,
}

/// A lane taper at one end of a road, where it meets a road with more or
/// fewer lanes: over `length_m`, each lane moves sideways by its shift (m,
/// + left, reached at the road end) and lanes that close narrow to nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Taper {
    pub length_m: f32,
    /// (shift, closes) per forward lane, then per backward lane, by index.
    pub forward: Vec<(f32, bool)>,
    pub backward: Vec<(f32, bool)>,
}

impl RoadGeometry {
    pub fn has_tapers(&self) -> bool {
        self.tapers.iter().any(Option::is_some)
    }

    /// How far into taper `end` (0 = start, 1 = end) arc length `s` is: 0
    /// outside it, rising smoothly to 1 at the road end.
    fn taper_ramp(&self, end: usize, s: f32) -> f32 {
        let Some(t) = &self.tapers[end] else {
            return 0.0;
        };
        let l = t.length_m.max(1e-3);
        let u = if end == 1 {
            (s - (self.centre.length() - l)) / l
        } else {
            (l - s) / l
        };
        let u = u.clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    }

    /// A lane's centre (m left of the centreline) and width at arc length `s`.
    pub fn lane_offset(&self, forward: bool, index: usize, s: f32) -> (f32, f32) {
        let base = if forward {
            &self.forward_offsets
        } else {
            &self.backward_offsets
        };
        let (mut offset, mut width) = (base[index], self.def.lane_width_m);
        for end in 0..2 {
            if let Some(t) = &self.tapers[end] {
                let (shift, closes) = if forward { &t.forward } else { &t.backward }[index];
                let r = self.taper_ramp(end, s);
                offset += shift * r;
                if closes {
                    width *= 1.0 - r;
                }
            }
        }
        (offset, width)
    }

    /// Carriageway edges at arc length `s`: (left, right), m left of the centreline.
    pub fn edges(&self, s: f32) -> (f32, f32) {
        if !self.has_tapers() {
            return (self.left_m, -self.right_m);
        }
        let lanes = (0..self.forward_offsets.len())
            .map(|i| self.lane_offset(true, i, s))
            .chain((0..self.backward_offsets.len()).map(|i| self.lane_offset(false, i, s)));
        lanes.fold((f32::MIN, f32::MAX), |(l, r), (o, w)| {
            (l.max(o + w / 2.0), r.min(o - w / 2.0))
        })
    }
}

/// Length of a taper that moves traffic `w` metres sideways at `kmh`
/// (MUTCD merging taper: L = W·S at 45 mph and above, W·S²/60 below; S in mph).
pub fn taper_length_m(w: f32, kmh: f32) -> f32 {
    let mph = kmh / 1.609;
    if mph >= 45.0 {
        w * mph
    } else {
        w * mph * mph / 60.0
    }
}

/// A junction (any node joined by two or more roads).
#[derive(Clone, Debug)]
pub struct Junction {
    pub node: usize,
    pub position: Vec2,
    /// Carriageway edge points of every road end, sorted around the junction.
    pub corners: Vec<Vec2>,
    /// How traffic is controlled here.
    pub control: crate::def::Control,
    /// With signals: the roads in each phase (approaches from opposite
    /// directions share one), in the order they get green.
    pub phases: Vec<Vec<usize>>,
    /// Each road meeting here: (road, where it meets the node (road end
    /// point), its direction away from the node).
    pub arms: Vec<(usize, Vec2, Vec2)>,
}

/// A traffic light's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Light {
    Green,
    Amber,
    Red,
}

impl Junction {
    /// The light shown at time `t` (s) to traffic arriving on `road`; `None`
    /// without signals.
    pub fn light(&self, road: usize, t: f64) -> Option<Light> {
        let crate::def::Control::Signals(timing) = &self.control else {
            return None;
        };
        let phase = self.phases.iter().position(|p| p.contains(&road))?;
        let slot = (timing.green_s + timing.amber_s + timing.all_red_s) as f64;
        let period = slot * self.phases.len() as f64;
        let local = (t + timing.offset_s as f64).rem_euclid(period);
        let now = (local / slot) as usize;
        let within = local - now as f64 * slot;
        Some(if now != phase {
            Light::Red
        } else if within < timing.green_s as f64 {
            Light::Green
        } else if within < (timing.green_s + timing.amber_s) as f64 {
            Light::Amber
        } else {
            Light::Red
        })
    }

    /// Seconds until the light shown to traffic on `road` next changes.
    pub fn time_left(&self, road: usize, t: f64) -> Option<f64> {
        let crate::def::Control::Signals(timing) = &self.control else {
            return None;
        };
        let phase = self.phases.iter().position(|p| p.contains(&road))?;
        let slot = (timing.green_s + timing.amber_s + timing.all_red_s) as f64;
        let period = slot * self.phases.len() as f64;
        let local = (t + timing.offset_s as f64).rem_euclid(period);
        let now = (local / slot) as usize;
        let within = local - now as f64 * slot;
        let (g, a) = (timing.green_s as f64, timing.amber_s as f64);
        Some(if now != phase {
            // Red until this phase's slot starts.
            let start = phase as f64 * slot;
            (start - local).rem_euclid(period)
        } else if within < g {
            g - within
        } else if within < g + a {
            g + a - within
        } else {
            period - within
        })
    }

    /// Whether traffic arriving on `road` must give way here (a priority
    /// junction where `road` is not a major road).
    pub fn gives_way(&self, road_id: &str) -> bool {
        match &self.control {
            crate::def::Control::Priority { major } => !major.iter().any(|m| m == road_id),
            _ => false,
        }
    }
}

/// Group a junction's arms into signal phases: arms whose directions are
/// within 30° of parallel or opposite share a phase.
fn signal_phases(arms: &[(usize, Vec2, Vec2)]) -> Vec<Vec<usize>> {
    let mut phases: Vec<(Vec<usize>, Vec2)> = Vec::new();
    for &(road, _, dir) in arms {
        match phases
            .iter_mut()
            .find(|(_, d)| d.dot(dir).abs() > 30f32.to_radians().cos())
        {
            Some((roads, _)) => roads.push(road),
            None => phases.push((vec![road], dir)),
        }
    }
    phases.into_iter().map(|(r, _)| r).collect()
}

#[derive(Clone, Debug)]
pub struct RoadNetwork {
    pub name: String,
    pub drive_side: DriveSide,
    pub roads: Vec<RoadGeometry>,
    pub lanes: Vec<Lane>,
    pub junctions: Vec<Junction>,
    /// Where pedestrians walk: sidewalks, crossings and corners.
    pub sidewalks: crate::sidewalk::SidewalkGraph,
    /// Bus stops, placed on their kerb-side lane.
    pub stops: Vec<Stop>,
    /// Street lamps along roads with sidewalks (scenery and night light).
    pub lamps: Vec<Lamp>,
    /// Road lanes by grid cell, for finding the lane under a point quickly.
    lane_grid: std::collections::BTreeMap<(i32, i32), Vec<LaneId>>,
}

/// A street lamp: a pole at the kerb with its arm over the road.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lamp {
    pub position: Vec2,
    /// Unit vector from the pole towards the road.
    pub toward_road: Vec2,
}

/// Distance between lamps on one side of a road, m (staggered sides).
pub const LAMP_SPACING_M: f32 = 30.0;

/// Lamps on both sidewalks of every road that has them, staggered, kept
/// clear of the junctions at the ends.
fn street_lamps(roads: &[RoadGeometry]) -> Vec<Lamp> {
    let mut out = Vec::new();
    for g in roads {
        if g.def.sidewalk_m <= 0.0 {
            continue;
        }
        let len = g.centre.length();
        for (side, edge, start) in [
            (1.0f32, g.left_m, 12.0f32),
            (-1.0, g.right_m, 12.0 + LAMP_SPACING_M / 2.0),
        ] {
            let mut s = start;
            while s < len - 12.0 {
                let (p, dir) = g.centre.sample(s);
                let left = Vec2::new(-dir.y, dir.x);
                out.push(Lamp {
                    position: p + left * side * (edge + 0.45),
                    toward_road: -left * side,
                });
                s += LAMP_SPACING_M;
            }
        }
    }
    out
}

/// Size of a lane-grid cell, m.
const GRID_M: f32 = 25.0;

fn cell_of(p: Vec2) -> (i32, i32) {
    ((p.x / GRID_M).floor() as i32, (p.y / GRID_M).floor() as i32)
}

/// Every cell a lane's path (grown by its half width and a margin) touches.
fn lane_grid(lanes: &[Lane]) -> std::collections::BTreeMap<(i32, i32), Vec<LaneId>> {
    let mut grid: std::collections::BTreeMap<(i32, i32), Vec<LaneId>> = Default::default();
    for (id, lane) in lanes.iter().enumerate() {
        if !matches!(lane.kind, LaneKind::Road { .. }) {
            continue;
        }
        let reach = lane.width_m / 2.0 + 0.6;
        for w in lane.path.points.windows(2) {
            let (lo, hi) = (
                w[0].min(w[1]) - Vec2::splat(reach),
                w[0].max(w[1]) + Vec2::splat(reach),
            );
            let (c0, c1) = (cell_of(lo), cell_of(hi));
            for x in c0.0..=c1.0 {
                for y in c0.1..=c1.1 {
                    let list = grid.entry((x, y)).or_default();
                    if list.last() != Some(&id) {
                        list.push(id);
                    }
                }
            }
        }
    }
    for list in grid.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    grid
}

/// A bus stop, placed: buses stop in `lane` at `s`.
#[derive(Clone, Debug)]
pub struct Stop {
    pub id: String,
    pub name: String,
    pub road: usize,
    pub lane: LaneId,
    /// Distance along the lane's path, m.
    pub s: f32,
    /// Where a bus's centre should be, and the way it faces.
    pub position: Vec2,
    pub direction: Vec2,
    /// Beside the road on the kerb side: where the sign and shelter stand.
    pub kerb: Vec2,
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
            let need = junction_clearance(ends, |ri| half_width(&full[ri].0));
            // Junctions get a margin for the crossing; a bend between two
            // roads only needs to clear the inside corner.
            let margin = if ends.len() == 2 { 0.2 } else { 1.0 };
            for (k, (ri, starts_here, _)) in ends.iter().enumerate() {
                let len = full[*ri].1.length();
                let t = (need[k] + margin).min(len * 0.45);
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
        for (ri, (r, line)) in full.iter().enumerate() {
            let centre = line.trim(trim_start[ri], trim_end[ri]);
            let w = r.lane_width_m;
            // Right-hand traffic: forward lanes on the right (negative offsets).
            let (right_m, left_m) = if side > 0.0 {
                (r.lanes_forward as f32 * w, r.lanes_backward as f32 * w)
            } else {
                (r.lanes_backward as f32 * w, r.lanes_forward as f32 * w)
            };
            // A one-way road's line is the middle of its carriageway (as in
            // OpenStreetMap), so shift its lanes to straddle it (+ = left).
            let shift = (right_m - left_m) / 2.0
                * (r.lanes_forward == 0 || r.lanes_backward == 0) as u8 as f32;
            let (left_m, right_m) = (left_m + shift, right_m - shift);
            roads.push(RoadGeometry {
                def: r.clone(),
                centre,
                left_m,
                right_m,
                forward_offsets: (0..r.lanes_forward)
                    .map(|i| shift - side * (i as f32 + 0.5) * w)
                    .collect(),
                backward_offsets: (0..r.lanes_backward)
                    .map(|i| shift + side * (i as f32 + 0.5) * w)
                    .collect(),
                tapers: [None, None],
                from_node: node_index[r.from.as_str()],
                to_node: node_index[r.to.as_str()],
            });
        }

        // Tapers where one road continues into another with a different
        // number of lanes: the wider road narrows (or widens) over a taper so
        // every lane meets its partner across the node.
        for ends in at_node.values().filter(|e| e.len() == 2) {
            add_tapers(&mut roads, [(ends[0].0, ends[0].1), (ends[1].0, ends[1].1)]);
        }
        for g in roads.iter_mut().filter(|g| g.has_tapers()) {
            let len = g.centre.length();
            let cap = if g.tapers[0].is_some() && g.tapers[1].is_some() {
                0.45
            } else {
                0.8
            };
            for t in g.tapers.iter_mut().flatten() {
                t.length_m = t.length_m.min(len * cap);
            }
        }

        let mut lanes = Vec::new();
        for (ri, g) in roads.iter().enumerate() {
            let r = &g.def;
            // Tapered roads are sampled every metre so the lanes bend smoothly.
            let fine = g.has_tapers().then(|| g.centre.resample(1.0));
            let path = |forward: bool, i: usize| match &fine {
                Some(c) => c.offset_with(|s| g.lane_offset(forward, i, s).0),
                None => g.centre.offset(if forward {
                    g.forward_offsets[i]
                } else {
                    g.backward_offsets[i]
                }),
            };
            for i in 0..r.lanes_forward {
                lanes.push(Lane {
                    name: format!("{}:f{i}", r.id),
                    kind: LaneKind::Road {
                        road: ri,
                        forward: true,
                        index: i,
                    },
                    path: path(true, i as usize),
                    width_m: r.lane_width_m,
                    speed_limit_kmh: r.speed_limit_kmh,
                    next: Vec::new(),
                    approach: None,
                    changes: Vec::new(),
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
                    path: path(false, i as usize).reversed(),
                    width_m: r.lane_width_m,
                    speed_limit_kmh: r.speed_limit_kmh,
                    next: Vec::new(),
                    approach: None,
                    changes: Vec::new(),
                });
            }
        }

        // Lanes of each road, and lane changes between neighbours.
        let mut road_lanes: Vec<Vec<LaneId>> = vec![Vec::new(); roads.len()];
        for (id, lane) in lanes.iter().enumerate() {
            if let LaneKind::Road { road, .. } = lane.kind {
                road_lanes[road].push(id);
            }
        }
        for ids in &road_lanes {
            for &a in ids {
                for &b in ids {
                    if let (
                        LaneKind::Road {
                            forward: fa,
                            index: ia,
                            ..
                        },
                        LaneKind::Road {
                            forward: fb,
                            index: ib,
                            ..
                        },
                    ) = (lanes[a].kind, lanes[b].kind)
                        && fa == fb
                        && ia.abs_diff(ib) == 1
                        && lanes[a].path.length() >= MIN_LANE_CHANGE_M
                    {
                        lanes[a].changes.push(b);
                    }
                }
            }
        }

        // Junction connectors.
        let mut junctions = Vec::new();
        for (&node, ends) in &at_node {
            let p = pos(&def.nodes[node].id).unwrap_or_default();
            if ends.len() >= 2 {
                let mut corners: Vec<Vec2> = Vec::new();
                for (ri, starts_here, _) in ends {
                    let g = &roads[*ri];
                    let at = if *starts_here { 0.0 } else { g.centre.length() };
                    let (pt, dir) = g.centre.sample(at);
                    let left = Vec2::new(-dir.y, dir.x);
                    let (l, r) = g.edges(at);
                    corners.push(pt + left * l);
                    corners.push(pt + left * r);
                }
                corners.sort_by(|a, b| (*a - p).to_angle().total_cmp(&(*b - p).to_angle()));
                let arms: Vec<(usize, Vec2, Vec2)> = ends
                    .iter()
                    .map(|&(ri, starts_here, _)| {
                        let g = &roads[ri];
                        let at = if starts_here { 0.0 } else { g.centre.length() };
                        let (pt, dir) = g.centre.sample(at);
                        (ri, pt, if starts_here { dir } else { -dir })
                    })
                    .collect();
                let control = def.nodes[node].control.clone();
                let phases = match control {
                    crate::def::Control::Signals(_) => signal_phases(&arms),
                    _ => Vec::new(),
                };
                junctions.push(Junction {
                    node,
                    position: p,
                    corners,
                    control,
                    phases,
                    arms,
                });
            }
            // Lanes arriving at / leaving this node.
            let arriving: Vec<LaneId> = lane_ids_at(&lanes, &road_lanes, ends, false);
            let leaving: Vec<LaneId> = lane_ids_at(&lanes, &road_lanes, ends, true);
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
                    // Where a road just continues (a bend, a lane drop), lane i
                    // flows into lane i. Straight on through a junction, each
                    // lane flows into the lane ahead at the same sideways
                    // position: lane for lane across a crossing, and into the
                    // outer lane where a slip road merges.
                    let in_line = || {
                        let end = lanes[inn].path.points.last().copied().unwrap_or(p);
                        let normal = Vec2::new(-t_out.y, t_out.x);
                        let side = |q: Vec2| (q - p).dot(normal);
                        leaving
                            .iter()
                            .copied()
                            .filter(|&o| {
                                matches!(lanes[o].kind, LaneKind::Road { road, .. } if road == r_out)
                            })
                            .min_by(|&a, &b| {
                                let d = |l: LaneId| {
                                    lanes[l]
                                        .path
                                        .points
                                        .first()
                                        .map_or(f32::MAX, |&q| (side(q) - side(end)).abs())
                                };
                                d(a).total_cmp(&d(b)).then(a.cmp(&b))
                            })
                            == Some(out)
                    };
                    let allowed = match turn {
                        // A gained lane opens from the outer lane before it.
                        _ if continuation => {
                            i_out == i_in.min(n_out - 1) || (i_in == n_in - 1 && i_out > i_in)
                        }
                        Turn::Straight => in_line(),
                        t if t == across => i_in == 0 && i_out == 0,
                        t if t == away => i_in == n_in - 1 && i_out == n_out - 1,
                        _ => false,
                    };
                    if !allowed {
                        continue;
                    }
                    let a = lanes[inn].path.points.last().copied().unwrap_or(p);
                    let b = lanes[out].path.points.first().copied().unwrap_or(p);
                    let path = connector(a, t_in, b, t_out, 10);
                    if min_radius(&path) < MIN_TURN_RADIUS_M {
                        continue; // tighter than a car can turn (a U-turn through a narrow median)
                    }
                    let id = lanes.len();
                    let speed = lanes[inn].speed_limit_kmh.min(if turn == Turn::Straight {
                        50.0
                    } else {
                        25.0
                    });
                    lanes.push(Lane {
                        name: format!("{}>{k}", def.nodes[node].id),
                        kind: LaneKind::Connector { node, turn },
                        path,
                        width_m: lanes[inn].width_m,
                        speed_limit_kmh: speed,
                        next: vec![out],
                        approach: Some(r_in),
                        changes: Vec::new(),
                    });
                    lanes[inn].next.push(id);
                    k += 1;
                }
            }
        }
        let arms_at: BTreeMap<usize, Vec<(usize, bool)>> = at_node
            .iter()
            .map(|(&n, ends)| {
                (
                    n,
                    ends.iter().map(|&(ri, starts, _)| (ri, starts)).collect(),
                )
            })
            .collect();
        let sidewalks = crate::sidewalk::SidewalkGraph::build(&roads, &arms_at);
        let kerb_side = -side; // right of travel in right-hand traffic
        let stops = def
            .stops
            .iter()
            .filter_map(|st| {
                let ri = roads.iter().position(|g| g.def.id == st.road)?;
                let g = &roads[ri];
                let n = if st.forward {
                    g.def.lanes_forward
                } else {
                    g.def.lanes_backward
                };
                let tag = if st.forward { 'f' } else { 'b' };
                let name = format!("{}:{tag}{}", g.def.id, n.checked_sub(1)?);
                let lane = lanes.iter().position(|l| l.name == name)?;
                let along =
                    (st.at_m - trim_start[ri]).clamp(2.0, (g.centre.length() - 2.0).max(2.0));
                let (on_centre, _) = g.centre.sample(along);
                let path = &lanes[lane].path;
                let (s, _, _) = path.project(on_centre);
                let (position, direction) = path.sample(s);
                let right = Vec2::new(direction.y, -direction.x) * -kerb_side;
                let kerb = position
                    + right * (g.def.lane_width_m / 2.0 + 0.6 + g.def.sidewalk_m.max(0.0) * 0.6);
                Some(Stop {
                    id: st.id.clone(),
                    name: st.name.clone(),
                    road: ri,
                    lane,
                    s,
                    position,
                    direction,
                    kerb,
                })
            })
            .collect();
        let lane_grid = lane_grid(&lanes);
        let lamps = street_lamps(&roads);
        RoadNetwork {
            lamps,
            name: def.name.clone(),
            drive_side: def.drive_side,
            roads,
            lanes,
            junctions,
            sidewalks,
            stops,
            lane_grid,
        }
    }

    /// The junction at node index `node`, if two or more roads meet there.
    pub fn junction_at(&self, node: usize) -> Option<&Junction> {
        self.junctions.iter().find(|j| j.node == node)
    }

    /// The light at time `t` for a vehicle about to take junction path
    /// `lane`; `None` if it is not a signalled junction path.
    pub fn light_for(&self, lane: LaneId, t: f64) -> Option<Light> {
        let l = &self.lanes[lane];
        let (LaneKind::Connector { node, .. }, Some(road)) = (l.kind, l.approach) else {
            return None;
        };
        self.junction_at(node)?.light(road, t)
    }

    pub fn lane(&self, name: &str) -> Option<LaneId> {
        self.lanes.iter().position(|l| l.name == name)
    }

    /// The road lane nearest to `p` within its width: (lane, arc length, lateral offset).
    pub fn lane_at(&self, p: Vec2) -> Option<(LaneId, f32, f32)> {
        let mut best: Option<(LaneId, f32, f32, f32)> = None;
        // Only lanes passing through this grid cell can be within reach.
        let candidates = self.lane_grid.get(&cell_of(p))?;
        for &i in candidates {
            let lane = &self.lanes[i];
            let (s, lat, dist) = lane.path.project(p);
            if dist <= lane.width_m / 2.0 + 0.5 && best.is_none_or(|b| dist < b.3) {
                best = Some((i, s, lat, dist));
            }
        }
        best.map(|(i, s, lat, _)| (i, s, lat))
    }

    /// Fastest lane sequence from `from` to any lane of road `to_road`
    /// (A* on travel time; straight-line distance at 50 km/h as the heuristic).
    /// Consecutive lanes are joined by a connector or, on the same road, by a
    /// lane change.
    pub fn route(&self, from: LaneId, to_road: &str) -> Option<Vec<LaneId>> {
        let goal_road = self.roads.iter().position(|r| r.def.id == to_road)?;
        let goal_point = self.roads[goal_road]
            .centre
            .sample(self.roads[goal_road].centre.length() / 2.0)
            .0;
        self.route_where(
            from,
            |l| matches!(self.lanes[l].kind, LaneKind::Road { road, .. } if road == goal_road),
            goal_point,
        )
    }

    /// Fastest lane sequence from `from` to lane `to` itself (changing lanes
    /// on the way where needed). `None` if it cannot be reached.
    pub fn route_to_lane(&self, from: LaneId, to: LaneId) -> Option<Vec<LaneId>> {
        if from == to {
            return Some(vec![from]);
        }
        let goal_point = self.lanes[to]
            .path
            .sample(self.lanes[to].path.length() / 2.0)
            .0;
        self.route_where(from, |l| l == to, goal_point)
    }

    /// A* from `from` to the first lane (other than `from`) where `is_goal`
    /// holds, guided towards `goal_point`.
    fn route_where(
        &self,
        from: LaneId,
        is_goal: impl Fn(LaneId) -> bool,
        goal_point: Vec2,
    ) -> Option<Vec<LaneId>> {
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
            let follow = self.lanes[l].next.iter().map(|&n| (n, cost(l)));
            let change = self.lanes[l]
                .changes
                .iter()
                .map(|&n| (n, LANE_CHANGE_PENALTY_S));
            for (n, step) in follow.chain(change) {
                let g = best[l] + step;
                if g < best[n] {
                    best[n] = g;
                    came[n] = l;
                    open.push(Item(g + h(n), n));
                }
            }
        }
        None
    }

    /// Every lane reachable from `from` (following junctions and lane changes).
    pub fn reachable(&self, from: LaneId) -> Vec<bool> {
        let mut seen = vec![false; self.lanes.len()];
        let mut stack = vec![from];
        while let Some(l) = stack.pop() {
            if std::mem::replace(&mut seen[l], true) {
                continue;
            }
            stack.extend(self.lanes[l].next.iter().chain(&self.lanes[l].changes));
        }
        seen
    }

    /// A good place to start driving: the road lane from which the most roads
    /// can be reached (ties go to the longer lane), searching the `candidates`
    /// longest lanes. Returns the lane and the roads reachable from it.
    pub fn best_start(&self, candidates: usize) -> Option<(LaneId, Vec<usize>)> {
        let mut lanes: Vec<LaneId> = (0..self.lanes.len())
            .filter(|&l| matches!(self.lanes[l].kind, LaneKind::Road { .. }))
            .collect();
        lanes.sort_by(|&a, &b| {
            self.lanes[b]
                .path
                .length()
                .total_cmp(&self.lanes[a].path.length())
                .then(a.cmp(&b))
        });
        let mut best: Option<(LaneId, Vec<usize>)> = None;
        for &l in lanes.iter().take(candidates) {
            let seen = self.reachable(l);
            let mut roads: Vec<usize> = seen
                .iter()
                .enumerate()
                .filter(|(_, s)| **s)
                .filter_map(|(i, _)| match self.lanes[i].kind {
                    LaneKind::Road { road, .. } => Some(road),
                    LaneKind::Connector { .. } => None,
                })
                .collect();
            roads.sort_unstable();
            roads.dedup();
            if best.as_ref().is_none_or(|b| roads.len() > b.1.len()) {
                best = Some((l, roads));
            }
        }
        best
    }

    /// The largest set of lanes that can all reach each other (the largest
    /// strongly connected component of the lane graph), in lane order. Good
    /// spawn points live here: from them, every lane in the set is routable.
    pub fn largest_connected(&self) -> Vec<LaneId> {
        let n = self.lanes.len();
        let succ = |l: LaneId| {
            self.lanes[l]
                .next
                .iter()
                .chain(&self.lanes[l].changes)
                .copied()
        };
        let mut pred: Vec<Vec<LaneId>> = vec![Vec::new(); n];
        for l in 0..n {
            for m in succ(l) {
                pred[m].push(l);
            }
        }
        // Kosaraju: finish order on the graph, then components on its reverse.
        let mut order = Vec::with_capacity(n);
        let mut seen = vec![false; n];
        for root in 0..n {
            if seen[root] {
                continue;
            }
            seen[root] = true;
            let mut stack = vec![(root, succ(root).collect::<Vec<_>>())];
            while let Some((l, rest)) = stack.last_mut() {
                if let Some(m) = rest.pop() {
                    if !seen[m] {
                        seen[m] = true;
                        stack.push((m, succ(m).collect()));
                    }
                } else {
                    order.push(*l);
                    stack.pop();
                }
            }
        }
        let mut comp = vec![usize::MAX; n];
        let mut sizes = Vec::new();
        for &root in order.iter().rev() {
            if comp[root] != usize::MAX {
                continue;
            }
            let c = sizes.len();
            let mut size = 0;
            let mut stack = vec![root];
            comp[root] = c;
            while let Some(l) = stack.pop() {
                size += 1;
                for &m in &pred[l] {
                    if comp[m] == usize::MAX {
                        comp[m] = c;
                        stack.push(m);
                    }
                }
            }
            sizes.push(size);
        }
        let Some(big) = (0..sizes.len()).max_by_key(|&c| (sizes[c], usize::MAX - c)) else {
            return Vec::new();
        };
        (0..n).filter(|&l| comp[l] == big).collect()
    }

    /// The path a vehicle drives along a lane sequence, starting `start_s`
    /// metres along the first lane. Lane changes blend smoothly into the next
    /// lane, spread evenly over the rest of the road.
    pub fn route_path(&self, route: &[LaneId], start_s: f32) -> Polyline {
        let mut pts = Vec::new();
        let mut i = 0;
        while i < route.len() {
            // A run of lanes on one road joined by lane changes.
            let mut j = i;
            while j + 1 < route.len() && self.lanes[route[j]].changes.contains(&route[j + 1]) {
                j += 1;
            }
            let from_s = if i == 0 { start_s } else { 0.0 };
            pts.extend(self.lane_change_path(&route[i..=j], from_s).points);
            i = j + 1;
        }
        Polyline::new(pts)
    }

    /// Path along `run` (neighbouring lanes of one road), from `from_s` on the
    /// first lane, changing lane `run.len() - 1` times with smoothstep blends.
    fn lane_change_path(&self, run: &[LaneId], from_s: f32) -> Polyline {
        let first = &self.lanes[run[0]].path;
        let len0 = first.length();
        let from_s = from_s.clamp(0.0, len0);
        if run.len() == 1 {
            return first.slice(from_s, len0);
        }
        // Position on lane `k` at the point level with `s` on the first lane.
        let on = |k: usize, s: f32| {
            let p = &self.lanes[run[k]].path;
            p.sample(s * p.length() / len0.max(1e-3)).0
        };
        let changes = run.len() - 1;
        let share = (len0 - from_s) / changes as f32;
        let blend = (share * 0.9).min(60.0);
        let n = (((len0 - from_s) / 1.0).ceil() as usize).max(2);
        Polyline::new(
            (0..=n)
                .map(|k| {
                    let s = from_s + (len0 - from_s) * k as f32 / n as f32;
                    let x = ((s - from_s) / share).min(changes as f32 - 1e-4);
                    let c = x.floor() as usize; // change in progress
                    let local = (x - c as f32) * share; // m into this share
                    let u = ((local - (share - blend) / 2.0) / blend).clamp(0.0, 1.0);
                    let w = u * u * (3.0 - 2.0 * u);
                    on(c, s).lerp(on(c + 1, s), w)
                })
                .collect(),
        )
    }
}

/// How far each road end at a node must be cut back so its edge clears its
/// neighbours' edges. `ends` are (road, starts here, direction leaving the
/// node); `half_width(road)` is the road's half width, m.
///
/// For roads at angle θ (counter-clockwise from road i to road
/// j), their facing edges cross at distance (w_i·cosθ + w_j) / sinθ along road
/// i: w_j at a right angle, more for sharp angles, less for wide ones. A kerb
/// of radius R between them starts R·cot(θ/2) further out. Roads almost
/// opposite each other (a straight continuation) never cross.
fn junction_clearance(ends: &[(usize, bool, Vec2)], half_width: impl Fn(usize) -> f32) -> Vec<f32> {
    let mut need = vec![0.0f32; ends.len()];
    // Every ordered pair: wide roads can reach past a narrow one between them.
    let pairs = (0..ends.len()).flat_map(|i| (0..ends.len()).map(move |j| (i, j)));
    for (i, j) in pairs.filter(|(i, j)| i != j) {
        let mut theta = ends[i].2.angle_to(ends[j].2);
        if theta <= 0.0 {
            theta += std::f32::consts::TAU;
        }
        if theta > 160f32.to_radians() {
            // Nearly in line (a road bending on at a node): no kerb to
            // round, but the lanes need room to bend through, or their
            // connector has no length and reads as an impossibly tight turn.
            let bend = std::f32::consts::PI - theta.min(std::f32::consts::PI);
            if bend > 1f32.to_radians() {
                let room = BEND_RADIUS_M * (bend / 2.0).tan();
                need[i] = need[i].max(room);
                need[j] = need[j].max(room);
            }
            continue;
        }
        let (wi, wj) = (half_width(ends[i].0), half_width(ends[j].0));
        let (sin, cos) = theta.sin_cos();
        // Then room for a rounded kerb between the two edges.
        let kerb = KERB_RADIUS_M / (theta / 2.0).tan();
        need[i] = need[i].max((wi * cos + wj) / sin + kerb);
        need[j] = need[j].max((wj * cos + wi) / sin + kerb);
    }
    need
}

/// Tightest radius of a path, m, from the turn between consecutive segments.
fn min_radius(path: &Polyline) -> f32 {
    let p = &path.points;
    (1..p.len().saturating_sub(1))
        .map(|i| {
            let (a, b) = (p[i] - p[i - 1], p[i + 1] - p[i]);
            let turn = a.angle_to(b).abs();
            if turn < 1e-6 {
                f32::INFINITY
            } else {
                (a.length() + b.length()) / 2.0 / turn
            }
        })
        .fold(f32::INFINITY, f32::min)
}

/// Add tapers where two roads meet end to end at a node (`ends`: road and
/// whether it starts there) with different lane counts in a direction of
/// travel. The wider road gets the taper at that end; each of its lanes
/// shifts to meet its partner on the narrower road (lane i meets lane i, the
/// extra outer lanes meet the outermost), and the extra lanes close.
fn add_tapers(roads: &mut [RoadGeometry], ends: [(usize, bool); 2]) {
    let offsets = |g: &RoadGeometry, forward: bool| {
        if forward {
            g.forward_offsets.clone()
        } else {
            g.backward_offsets.clone()
        }
    };
    for (x, y) in [(ends[0], ends[1]), (ends[1], ends[0])] {
        if x.0 == y.0 {
            continue;
        }
        // Traffic arrives on road x and leaves on road y.
        let (x_fwd, y_fwd) = (!x.1, y.1);
        let count = |g: &RoadGeometry, fwd: bool| {
            if fwd {
                g.forward_offsets.len()
            } else {
                g.backward_offsets.len()
            }
        };
        let (n_in, n_out) = (count(&roads[x.0], x_fwd), count(&roads[y.0], y_fwd));
        if n_in == n_out || n_in == 0 || n_out == 0 {
            continue;
        }
        // The wider road (`wide`, lanes in direction `fwd`) tapers towards `narrow`.
        let (wide, narrow, wide_fwd, narrow_fwd, n_wide, n_narrow) = if n_in > n_out {
            (x, y, x_fwd, y_fwd, n_in, n_out)
        } else {
            (y, x, y_fwd, x_fwd, n_out, n_in)
        };
        let (gw, gn) = (&roads[wide.0], &roads[narrow.0]);
        // Both centrelines meet at the node, so lanes line up by their offset
        // from it; flip the narrow road's offsets if it points the other way.
        let flip = if wide.1 != narrow.1 { 1.0 } else { -1.0 };
        let (wide_offs, narrow_offs) = (offsets(gw, wide_fwd), offsets(gn, narrow_fwd));
        let w = gw.def.lane_width_m;
        let mut moves = Vec::with_capacity(n_wide);
        let mut widest_move: f32 = 0.0;
        for (i, own) in wide_offs.iter().enumerate() {
            let shift = flip * narrow_offs[i.min(n_narrow - 1)] - own;
            let closes = i >= n_narrow;
            widest_move = widest_move
                .max(shift.abs())
                .max(if closes { w } else { 0.0 });
            moves.push((shift, closes));
        }
        let length = taper_length_m(widest_move, gw.def.speed_limit_kmh).max(10.0);
        let g = &mut roads[wide.0];
        let end = if wide.1 { 0 } else { 1 };
        let (nf, nb) = (g.forward_offsets.len(), g.backward_offsets.len());
        let t = g.tapers[end].get_or_insert_with(|| Taper {
            length_m: 0.0,
            forward: vec![(0.0, false); nf],
            backward: vec![(0.0, false); nb],
        });
        t.length_m = t.length_m.max(length);
        if wide_fwd {
            t.forward = moves;
        } else {
            t.backward = moves;
        }
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
    road_lanes: &[Vec<LaneId>],
    ends: &[(usize, bool, Vec2)],
    leaving: bool,
) -> Vec<LaneId> {
    let mut out = Vec::new();
    for (ri, starts_here, _) in ends {
        for &id in &road_lanes[*ri] {
            if let LaneKind::Road { forward, .. } = lanes[id].kind {
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
            control: Default::default(),
        };
        RoadNetworkDef {
            stops: Vec::new(),
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
        let path = net.route_path(&route, 0.0);
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
            control: Default::default(),
        };
        let mut wide = road("a", "p", "q");
        wide.lanes_forward = 2;
        let def = RoadNetworkDef {
            stops: Vec::new(),
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

    #[test]
    fn junction_clearance_grows_for_sharp_angles() {
        let w = |_| 7.0;
        let at = |deg: f32| Vec2::from_angle(deg.to_radians());
        // Crossroads: each end clears its neighbour's half width.
        let cross = [
            (0, true, at(0.0)),
            (1, true, at(90.0)),
            (2, true, at(180.0)),
            (3, true, at(270.0)),
        ];
        for n in junction_clearance(&cross, w) {
            assert!((n - 7.0 - KERB_RADIUS_M).abs() < 1e-3, "{n}");
        }
        // A sharp 60° fork needs far more room; a straight bend needs none.
        let fork = [(0, true, at(0.0)), (1, true, at(60.0))];
        assert!(junction_clearance(&fork, w)[0] > 12.0 + KERB_RADIUS_M);
        let straight = [(0, true, at(0.0)), (1, true, at(179.0))];
        assert_eq!(junction_clearance(&straight, w), vec![0.0, 0.0]);
    }

    /// Three nodes in a line; `a` from p to q, `b` from q to r, one-way.
    fn lane_drop(lanes_a: u8, lanes_b: u8) -> RoadNetworkDef {
        let node = |id: &str, x: f32| NodeDef {
            id: id.into(),
            position: (x, 0.0),
            control: Default::default(),
        };
        let one_way = |id: &str, from: &str, to: &str, n: u8| RoadDef {
            lanes_forward: n,
            lanes_backward: 0,
            ..road(id, from, to)
        };
        RoadNetworkDef {
            stops: Vec::new(),
            name: "drop".into(),
            drive_side: DriveSide::Right,
            nodes: vec![node("p", 0.0), node("q", 200.0), node("r", 400.0)],
            roads: vec![
                one_way("a", "p", "q", lanes_a),
                one_way("b", "q", "r", lanes_b),
            ],
        }
    }

    #[test]
    fn a_dropped_lane_tapers_into_its_neighbour() {
        let net = RoadNetwork::build(&lane_drop(2, 1));
        let a = &net.roads[0];
        let t = a.tapers[1].as_ref().expect("a taper where a meets b");
        assert!(a.tapers[0].is_none() && !net.roads[1].has_tapers());
        // MUTCD at 50 km/h for one 3.5 m lane: 3.5 × 31.1² / 60 ≈ 56 m.
        assert!((t.length_m - 56.3).abs() < 0.5, "taper {} m", t.length_m);
        assert!(t.forward[1].1, "the outer lane closes");
        // Full width before the taper, one lane at the node.
        let len = a.centre.length();
        let (l0, r0) = a.edges(10.0);
        let (l1, r1) = a.edges(len);
        assert!(((l0 - r0) - 7.0).abs() < 1e-3 && ((l1 - r1) - 3.5).abs() < 1e-3);
        // Both lanes end where b's lane starts, so either can carry on.
        let b0 = net.lanes[net.lane("b:f0").unwrap()].path.points[0];
        for name in ["a:f0", "a:f1"] {
            let lane = &net.lanes[net.lane(name).unwrap()];
            let end = *lane.path.points.last().unwrap();
            assert!(
                (end.y - b0.y).abs() < 0.01,
                "{name} ends at {end}, b at {b0}"
            );
            assert!(
                net.route(net.lane(name).unwrap(), "b").is_some(),
                "{name} reaches b"
            );
        }
        // The road surface still faces up through the taper.
        let m = crate::mesh::build_meshes(&net);
        for t in m.asphalt.indices.chunks(3) {
            let p = |i: u32| glam::Vec3::from(m.asphalt.positions[i as usize]);
            assert!((p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).z >= -1e-4);
        }
    }

    #[test]
    fn a_gained_lane_opens_from_the_outer_lane() {
        let net = RoadNetwork::build(&lane_drop(1, 2));
        let b = &net.roads[1];
        assert!(b.tapers[0].is_some(), "b widens at its start");
        let into: Vec<&str> = net.lanes[net.lane("a:f0").unwrap()]
            .next
            .iter()
            .map(|&c| net.lanes[net.lanes[c].next[0]].name.as_str())
            .collect();
        assert_eq!(into, ["b:f0", "b:f1"], "both of b's lanes can be entered");
    }

    #[test]
    fn taper_lengths_follow_mutcd() {
        assert!(
            (taper_length_m(3.6, 100.0) - 223.7).abs() < 0.5,
            "W·S above 45 mph"
        );
        assert!(
            (taper_length_m(3.6, 50.0) - 57.9).abs() < 0.5,
            "W·S²/60 below"
        );
    }

    #[test]
    fn signals_alternate_between_opposite_pairs() {
        let mut def = crossroads();
        def.nodes[0].control = crate::def::Control::Signals(crate::def::SignalTiming::default());
        let net = RoadNetwork::build(&def);
        let j = net.junction_at(0).expect("the centre is a junction");
        assert_eq!(j.phases.len(), 2, "{:?}", j.phases);
        let road = |id: &str| net.roads.iter().position(|r| r.def.id == id).unwrap();
        let (ns, ew) = (road("north"), road("east"));
        // Phase 0 (whichever pair came first) is green for 20 s, amber 3, red after.
        let first = j.phases[0][0];
        let other = if j.phases[0].contains(&ns) { ew } else { ns };
        assert_eq!(j.light(first, 5.0), Some(Light::Green));
        assert_eq!(j.light(first, 21.0), Some(Light::Amber));
        assert_eq!(j.light(first, 24.0), Some(Light::Red));
        assert_eq!(j.light(other, 5.0), Some(Light::Red));
        assert_eq!(j.light(other, 26.0), Some(Light::Green));
        // Never green for both at once.
        for k in 0..500 {
            let t = k as f64 * 0.1;
            assert!(
                !(j.light(ns, t) == Some(Light::Green) && j.light(ew, t) == Some(Light::Green))
            );
        }
        // Junction paths know their light.
        let path = net.lanes[net.lane("west:f0").unwrap()].next[0];
        assert!(net.light_for(path, 5.0).is_some());
    }
}
