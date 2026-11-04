//! The `.road.ron` format: junction nodes and roads between them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wp_core::Diagnostic;
use wp_data::Loaded;

/// A road network: `roads/<name>.road.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoadNetworkDef {
    pub name: String,
    /// Which side of the road traffic drives on.
    #[serde(default)]
    pub drive_side: DriveSide,
    /// Junctions and road ends.
    pub nodes: Vec<NodeDef>,
    pub roads: Vec<RoadDef>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum DriveSide {
    #[default]
    Right,
    Left,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDef {
    pub id: String,
    /// (x east, y north), m.
    pub position: (f32, f32),
    /// Who goes first at this node: take turns (default), traffic signals, or
    /// major roads with priority.
    #[serde(default)]
    pub control: Control,
}

/// How traffic is controlled at a junction.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Control {
    /// Vehicles take turns: first to reach the junction goes, crossing
    /// paths wait.
    #[default]
    Auto,
    /// Traffic lights. Approaches from opposite directions share a phase;
    /// phases run in turn.
    Signals(SignalTiming),
    /// Roads in `major` have priority; traffic from the others gives way.
    Priority { major: Vec<String> },
}

/// Traffic-light timing for one junction.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignalTiming {
    /// Green time of each phase, s.
    #[serde(default = "default_green_s")]
    pub green_s: f32,
    /// Amber after each green, s.
    #[serde(default = "default_amber_s")]
    pub amber_s: f32,
    /// All approaches red between phases, s.
    #[serde(default = "default_all_red_s")]
    pub all_red_s: f32,
    /// Shift of the cycle start, s (to coordinate neighbouring junctions).
    #[serde(default)]
    pub offset_s: f32,
}

impl Default for SignalTiming {
    fn default() -> Self {
        Self {
            green_s: default_green_s(),
            amber_s: default_amber_s(),
            all_red_s: default_all_red_s(),
            offset_s: 0.0,
        }
    }
}

fn default_green_s() -> f32 {
    20.0
}
fn default_amber_s() -> f32 {
    3.0
}
fn default_all_red_s() -> f32 {
    2.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoadDef {
    pub id: String,
    /// Street name, for display.
    #[serde(default)]
    pub name: Option<String>,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub shape: RoadShape,
    /// Lanes running from `from` to `to`.
    #[serde(default = "one")]
    pub lanes_forward: u8,
    /// Lanes running from `to` back to `from` (0 for a one-way road).
    #[serde(default = "one")]
    pub lanes_backward: u8,
    #[serde(default = "lane_width")]
    pub lane_width_m: f32,
    #[serde(default = "speed_limit")]
    pub speed_limit_kmh: f32,
    /// Sidewalk width on each side, m (0 for none).
    #[serde(default)]
    pub sidewalk_m: f32,
}

fn one() -> u8 {
    1
}
fn lane_width() -> f32 {
    3.5
}
fn speed_limit() -> f32 {
    50.0
}

/// The shape of a road's centreline between its two nodes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum RoadShape {
    #[default]
    Straight,
    /// A circular arc; positive radius bends left going from `from` to `to`.
    Arc { radius_m: f32 },
    /// A smooth curve (Catmull-Rom spline) through these points, in order.
    Curve { through: Vec<(f32, f32)> },
    /// Straight segments through these points (as imported from maps).
    Polyline { points: Vec<(f32, f32)> },
}

impl RoadDef {
    pub fn total_lanes(&self) -> u8 {
        self.lanes_forward + self.lanes_backward
    }
    pub fn carriageway_width(&self) -> f32 {
        self.total_lanes() as f32 * self.lane_width_m
    }
}

impl RoadNetworkDef {
    pub fn node(&self, id: &str) -> Option<&NodeDef> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn validate(file: &Loaded<RoadNetworkDef>) -> Vec<Diagnostic> {
        let net = &file.value;
        let mut out = Vec::new();
        for (i, n) in net.nodes.iter().enumerate() {
            if net.nodes[..i].iter().any(|o| o.id == n.id) {
                out.push(file.error_at(
                    "WP-N001",
                    "nodes",
                    format!("node `{}` is defined twice", n.id),
                ));
            }
        }
        for (i, r) in net.roads.iter().enumerate() {
            if net.roads[..i].iter().any(|o| o.id == r.id) {
                out.push(file.error_at(
                    "WP-N002",
                    "roads",
                    format!("road `{}` is defined twice", r.id),
                ));
            }
            for end in [&r.from, &r.to] {
                if net.node(end).is_none() {
                    out.push(
                        file.error_at(
                            "WP-N003",
                            "roads",
                            format!(
                                "road `{}` refers to node `{end}`, which does not exist",
                                r.id
                            ),
                        )
                        .help("add it to `nodes`"),
                    );
                }
            }
            if r.from == r.to {
                out.push(file.error_at(
                    "WP-N004",
                    "roads",
                    format!("road `{}` starts and ends at the same node", r.id),
                ));
            }
            if r.total_lanes() == 0 {
                out.push(file.error_at(
                    "WP-N005",
                    "roads",
                    format!("road `{}` has no lanes", r.id),
                ));
            }
            if !(2.0..=6.0).contains(&r.lane_width_m)
                || !(5.0..=200.0).contains(&r.speed_limit_kmh)
                || r.sidewalk_m < 0.0
            {
                out.push(
                    file.error_at(
                        "WP-N006",
                        "roads",
                        format!("road `{}` has an out-of-range dimension", r.id),
                    )
                    .expected("lane_width_m 2–6, speed_limit_kmh 5–200, sidewalk_m ≥ 0"),
                );
            }
            if let (Some(a), Some(b), RoadShape::Arc { radius_m }) =
                (net.node(&r.from), net.node(&r.to), &r.shape)
            {
                let chord = ((a.position.0 - b.position.0).powi(2)
                    + (a.position.1 - b.position.1).powi(2))
                .sqrt();
                if radius_m.abs() < chord / 2.0 + 1e-3 {
                    out.push(
                        file.error_at(
                            "WP-N007",
                            "radius_m",
                            format!("road `{}`: radius too small for its endpoints", r.id),
                        )
                        .expected(format!(
                            "|radius_m| ≥ half the distance between the nodes ({:.1})",
                            chord / 2.0
                        )),
                    );
                }
            }
        }
        for n in &net.nodes {
            match &n.control {
                Control::Auto => {}
                Control::Signals(t) => {
                    let ok = (5.0..=180.0).contains(&t.green_s)
                        && (2.0..=8.0).contains(&t.amber_s)
                        && (0.0..=10.0).contains(&t.all_red_s)
                        && t.offset_s.is_finite();
                    if !ok {
                        out.push(
                            file.error_at(
                                "WP-N009",
                                "green_s",
                                format!("node `{}`: signal timing out of range", n.id),
                            )
                            .expected("green_s 5–180, amber_s 2–8, all_red_s 0–10"),
                        );
                    }
                }
                Control::Priority { major } => {
                    for m in major {
                        let meets = net
                            .roads
                            .iter()
                            .any(|r| &r.id == m && (r.from == n.id || r.to == n.id));
                        if !meets {
                            out.push(
                                file.error_at(
                                    "WP-N008",
                                    "major",
                                    format!("node `{}`: major road `{m}` does not meet it", n.id),
                                )
                                .help("list roads that start or end at this node"),
                            );
                        }
                    }
                }
            }
        }
        out
    }
}
