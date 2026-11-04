//! OpenDRIVE import (ASAM OpenDRIVE 1.4–1.8, `.xodr`) → [`RoadNetworkDef`].
//!
//! Road reference lines (line, arc, spiral, poly3, paramPoly3) are sampled
//! exactly, shifted by `laneOffset` and placed where our model expects a
//! road's line: the divider between directions, or the middle of a one-way
//! carriageway. Each lane section becomes its own road, so lane counts can
//! change along a road. Junctions become nodes; their connecting roads are
//! not copied, because the network builds its own junction paths. Elevation,
//! superelevation and road marks are ignored (the engine is planar for now).

use crate::def::{DriveSide, NodeDef, RoadDef, RoadNetworkDef, RoadShape};
use quick_xml::events::{BytesStart, Event};
use std::collections::BTreeMap;

/// What an import found, for the report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportStats {
    pub roads_in_file: usize,
    pub junctions_in_file: usize,
    /// Roads inside junctions, replaced by the network's own junction paths.
    pub connecting_roads: usize,
    /// Roads without driving lanes (footpaths, borders only).
    pub skipped_roads: usize,
    pub roads: usize,
    /// How far the coordinates were shifted to centre them, m.
    pub shift: (f64, f64),
    pub extent_m: (f32, f32),
    /// Things the import could not represent, once each.
    pub notes: Vec<String>,
}

#[derive(Clone, Debug)]
enum Shape {
    Line,
    Arc(f64),
    Spiral(f64, f64),
    Poly3([f64; 4]),
    ParamPoly3 {
        u: [f64; 4],
        v: [f64; 4],
        normalized: bool,
    },
}

#[derive(Clone, Debug)]
struct Geometry {
    s: f64,
    x: f64,
    y: f64,
    hdg: f64,
    length: f64,
    shape: Shape,
}

/// a + b·ds + c·ds² + d·ds³ from `s`.
#[derive(Clone, Copy, Debug)]
struct Cubic {
    s: f64,
    k: [f64; 4],
}

impl Cubic {
    fn at(&self, s: f64) -> f64 {
        let d = s - self.s;
        self.k[0] + d * (self.k[1] + d * (self.k[2] + d * self.k[3]))
    }
}

/// The cubic in force at `s` (the last one starting at or before it).
fn cubic_at(list: &[Cubic], s: f64) -> f64 {
    list.iter()
        .rev()
        .find(|c| c.s <= s + 1e-9)
        .or(list.first())
        .map_or(0.0, |c| c.at(s))
}

#[derive(Clone, Debug, Default)]
struct Lane {
    id: i32,
    kind: String,
    /// Width records, `s` relative to the section start.
    widths: Vec<Cubic>,
}

impl Lane {
    fn drivable(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "driving"
                | "entry"
                | "exit"
                | "onRamp"
                | "offRamp"
                | "connectingRamp"
                | "bidirectional"
        )
    }
}

#[derive(Clone, Debug, Default)]
struct Section {
    s: f64,
    lanes: Vec<Lane>,
}

#[derive(Clone, Debug)]
struct Link {
    junction: bool,
    id: String,
    /// Contact at the other road's start (else its end).
    at_start: bool,
}

#[derive(Clone, Debug, Default)]
struct Road {
    id: String,
    name: Option<String>,
    junction: String,
    length: f64,
    left_hand: Option<bool>,
    pred: Option<Link>,
    succ: Option<Link>,
    geometry: Vec<Geometry>,
    offsets: Vec<Cubic>,
    sections: Vec<Section>,
    speed_kmh: Option<f32>,
}

fn attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| {
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.to_string())
        })
}

fn num(e: &BytesStart, key: &str) -> f64 {
    attr(e, key)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0.0)
}

fn cubic(e: &BytesStart, s_key: &str) -> Cubic {
    Cubic {
        s: num(e, s_key),
        k: [num(e, "a"), num(e, "b"), num(e, "c"), num(e, "d")],
    }
}

fn speed_kmh(e: &BytesStart) -> Option<f32> {
    let max: f32 = attr(e, "max")?.trim().parse().ok()?;
    Some(match attr(e, "unit").as_deref() {
        Some("mph") => max * 1.609,
        Some("m/s") => max * 3.6,
        _ => max,
    })
}

/// Parse the roads and the number of junctions.
fn parse(xml: &str) -> Result<(Vec<Road>, usize, bool), String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut roads = Vec::new();
    let mut road: Option<Road> = None;
    let mut junctions = 0;
    let mut saw_root = false;
    let mut file_lht = None;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("invalid XML at byte {}: {e}", reader.buffer_position()))?;
        let (e, empty) = match event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) => {
                if e.name().as_ref() == "road"
                    && let Some(r) = road.take()
                {
                    roads.push(r);
                }
                stack.pop();
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let name = e.name().as_ref().to_string();
        let parent = stack.last().map(String::as_str).unwrap_or("");
        let grandparent = stack.len().checked_sub(2).map_or("", |i| stack[i].as_str());
        match (name.as_str(), parent) {
            ("OpenDRIVE", _) => saw_root = true,
            ("header", _) => {
                if let Some(rule) = attr(&e, "rule") {
                    file_lht = Some(rule == "LHT");
                }
            }
            ("junction", "OpenDRIVE") => junctions += 1,
            ("road", "OpenDRIVE") => {
                road = Some(Road {
                    id: attr(&e, "id").unwrap_or_default(),
                    name: attr(&e, "name").filter(|n| !n.trim().is_empty()),
                    junction: attr(&e, "junction").unwrap_or_else(|| "-1".into()),
                    length: num(&e, "length"),
                    left_hand: attr(&e, "rule").map(|r| r == "LHT").or(file_lht),
                    ..Default::default()
                });
            }
            ("predecessor" | "successor", "link") if grandparent == "road" => {
                if let Some(r) = road.as_mut() {
                    let link = Link {
                        junction: attr(&e, "elementType").as_deref() == Some("junction"),
                        id: attr(&e, "elementId").unwrap_or_default(),
                        at_start: attr(&e, "contactPoint").as_deref() != Some("end"),
                    };
                    if name == "predecessor" {
                        r.pred = Some(link);
                    } else {
                        r.succ = Some(link);
                    }
                }
            }
            ("speed", "type") if grandparent == "road" => {
                if let Some(r) = road.as_mut()
                    && r.speed_kmh.is_none()
                {
                    r.speed_kmh = speed_kmh(&e);
                }
            }
            ("geometry", "planView") => {
                if let Some(r) = road.as_mut() {
                    r.geometry.push(Geometry {
                        s: num(&e, "s"),
                        x: num(&e, "x"),
                        y: num(&e, "y"),
                        hdg: num(&e, "hdg"),
                        length: num(&e, "length"),
                        shape: Shape::Line,
                    });
                }
            }
            ("line" | "arc" | "spiral" | "poly3" | "paramPoly3", "geometry") => {
                if let Some(g) = road.as_mut().and_then(|r| r.geometry.last_mut()) {
                    let k = |p: &str| ["a", "b", "c", "d"].map(|c| num(&e, &format!("{c}{p}")));
                    g.shape = match name.as_str() {
                        "arc" => Shape::Arc(num(&e, "curvature")),
                        "spiral" => Shape::Spiral(num(&e, "curvStart"), num(&e, "curvEnd")),
                        "poly3" => Shape::Poly3(k("")),
                        "paramPoly3" => Shape::ParamPoly3 {
                            u: k("U"),
                            v: k("V"),
                            normalized: attr(&e, "pRange").as_deref() != Some("arcLength"),
                        },
                        _ => Shape::Line,
                    };
                }
            }
            ("laneOffset", "lanes") => {
                if let Some(r) = road.as_mut() {
                    r.offsets.push(cubic(&e, "s"));
                }
            }
            ("laneSection", "lanes") => {
                if let Some(r) = road.as_mut() {
                    r.sections.push(Section {
                        s: num(&e, "s"),
                        lanes: Vec::new(),
                    });
                }
            }
            ("lane", "left" | "right" | "center") => {
                if let Some(sec) = road.as_mut().and_then(|r| r.sections.last_mut()) {
                    sec.lanes.push(Lane {
                        id: attr(&e, "id")
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0),
                        kind: attr(&e, "type").unwrap_or_default(),
                        widths: Vec::new(),
                    });
                }
            }
            ("width", "lane") => {
                if let Some(lane) = road
                    .as_mut()
                    .and_then(|r| r.sections.last_mut())
                    .and_then(|s| s.lanes.last_mut())
                {
                    lane.widths.push(cubic(&e, "sOffset"));
                }
            }
            _ => {}
        }
        if !empty {
            stack.push(name);
        }
    }
    if !saw_root {
        return Err("not an OpenDRIVE file (no <OpenDRIVE> element)".into());
    }
    Ok((roads, junctions, file_lht.unwrap_or(false)))
}

/// Points along a geometry record: (s, x, y, heading), about every `step` m
/// on curves (only the ends on straight lines).
fn sample(g: &Geometry, out: &mut Vec<(f64, f64, f64, f64)>) {
    let (sin0, cos0) = g.hdg.sin_cos();
    let local = |u: f64, v: f64| (g.x + u * cos0 - v * sin0, g.y + u * sin0 + v * cos0);
    let len = g.length.max(0.0);
    match &g.shape {
        Shape::Line => {
            out.push((g.s, g.x, g.y, g.hdg));
            out.push((g.s + len, g.x + len * cos0, g.y + len * sin0, g.hdg));
        }
        Shape::Arc(k) if k.abs() < 1e-9 => sample(
            &Geometry {
                shape: Shape::Line,
                ..g.clone()
            },
            out,
        ),
        Shape::Arc(k) => {
            // At most 2 m or 2° per step.
            let step = (0.035 / k.abs()).clamp(0.2, 2.0);
            let n = ((len / step).ceil() as usize).max(1);
            for i in 0..=n {
                let s = len * i as f64 / n as f64;
                let th = g.hdg + k * s;
                out.push((
                    g.s + s,
                    g.x + (th.sin() - sin0) / k,
                    g.y - (th.cos() - cos0) / k,
                    th,
                ));
            }
        }
        Shape::Spiral(c0, c1) => {
            // Integrate the heading (curvature linear in s) in small steps.
            let rate = if len > 0.0 { (c1 - c0) / len } else { 0.0 };
            let fine = 0.05;
            let n = ((len / fine).ceil() as usize).max(1);
            let h = len / n as f64;
            let every = ((1.0 / h).round() as usize).max(1);
            let (mut x, mut y) = (g.x, g.y);
            out.push((g.s, x, y, g.hdg));
            for i in 1..=n {
                let mid = (i as f64 - 0.5) * h;
                let th = g.hdg + c0 * mid + 0.5 * rate * mid * mid;
                x += h * th.cos();
                y += h * th.sin();
                if i % every == 0 || i == n {
                    let s = i as f64 * h;
                    out.push((g.s + s, x, y, g.hdg + c0 * s + 0.5 * rate * s * s));
                }
            }
        }
        Shape::Poly3(k) => {
            // v(u) in local coordinates; step u until the arc length is used up.
            let v = |u: f64| k[0] + u * (k[1] + u * (k[2] + u * k[3]));
            let dv = |u: f64| k[1] + u * (2.0 * k[2] + u * 3.0 * k[3]);
            let (mut u, mut s, du) = (0.0, 0.0, 0.05);
            let mut next = 0.0;
            while s < len {
                if s >= next {
                    let (x, y) = local(u, v(u));
                    out.push((g.s + s, x, y, g.hdg + dv(u).atan()));
                    next += 1.0;
                }
                s += du * (1.0 + dv(u + du / 2.0).powi(2)).sqrt();
                u += du;
            }
            let (x, y) = local(u, v(u));
            out.push((g.s + len, x, y, g.hdg + dv(u).atan()));
        }
        Shape::ParamPoly3 { u, v, normalized } => {
            let p_end = if *normalized { 1.0 } else { len };
            let n = ((len / 1.0).ceil() as usize).max(2);
            let f = |k: &[f64; 4], p: f64| k[0] + p * (k[1] + p * (k[2] + p * k[3]));
            let df = |k: &[f64; 4], p: f64| k[1] + p * (2.0 * k[2] + p * 3.0 * k[3]);
            for i in 0..=n {
                let p = p_end * i as f64 / n as f64;
                let (x, y) = local(f(u, p), f(v, p));
                let th = g.hdg + df(v, p).atan2(df(u, p));
                out.push((g.s + len * i as f64 / n as f64, x, y, th));
            }
        }
    }
}

/// The road's reference line, then shifted by `laneOffset`: (s, x, y, heading).
fn reference_line(road: &Road) -> Vec<(f64, f64, f64, f64)> {
    let mut geoms = road.geometry.clone();
    geoms.sort_by(|a, b| a.s.total_cmp(&b.s));
    let mut pts: Vec<(f64, f64, f64, f64)> = Vec::new();
    for g in &geoms {
        let mut part = Vec::new();
        sample(g, &mut part);
        for p in part {
            if pts
                .last()
                .is_none_or(|q| (p.1 - q.1).hypot(p.2 - q.2) > 1e-3)
            {
                pts.push(p);
            }
        }
    }
    pts.into_iter()
        .map(|(s, x, y, h)| {
            let off = cubic_at(&road.offsets, s);
            (s, x - off * h.sin(), y + off * h.cos(), h)
        })
        .collect()
}

/// A lane section, reduced to what our road model holds.
struct Profile {
    left: u8,
    right: u8,
    lane_width_m: f32,
    sidewalk_m: f32,
    /// Where the line between directions (or the middle of a one-way road)
    /// sits, m left of the lane-offset line.
    divider_m: f64,
}

fn profile(sec: &Section, length: f64) -> Profile {
    let mid = length / 2.0;
    let width = |l: &Lane| cubic_at(&l.widths, mid).max(0.0);
    // Edges of the driving lanes on each side, measured outwards from lane 0.
    let side = |sign: i32| {
        let mut lanes: Vec<&Lane> = sec.lanes.iter().filter(|l| l.id.signum() == sign).collect();
        lanes.sort_by_key(|l| l.id.abs());
        let (mut edge, mut inner, mut outer, mut count, mut widths) = (0.0, None, 0.0, 0u8, 0.0);
        let mut sidewalk: f64 = 0.0;
        for l in lanes {
            let w = width(l);
            if l.drivable() && w > 0.5 {
                inner.get_or_insert(edge);
                outer = edge + w;
                count += 1;
                widths += w;
            }
            if l.kind == "sidewalk" {
                sidewalk = sidewalk.max(w);
            }
            edge += w;
        }
        (count, inner.unwrap_or(0.0), outer, widths, sidewalk)
    };
    let (left, l_in, l_out, l_w, l_walk) = side(1);
    let (right, r_in, r_out, r_w, r_walk) = side(-1);
    let total = left + right;
    let divider_m = match (left, right) {
        (0, 0) => 0.0,
        (_, 0) => (l_in + l_out) / 2.0,
        (0, _) => -(r_in + r_out) / 2.0,
        _ => (l_in - r_in) / 2.0,
    };
    Profile {
        left,
        right,
        lane_width_m: if total > 0 {
            ((l_w + r_w) / total as f64) as f32
        } else {
            3.5
        },
        sidewalk_m: l_walk.max(r_walk) as f32,
        divider_m,
    }
}

/// Convert OpenDRIVE XML to a road network named `name`.
pub fn import(xml: &str, name: &str) -> Result<(RoadNetworkDef, ImportStats), String> {
    let (roads, junctions, file_lht) = parse(xml)?;
    let mut stats = ImportStats {
        roads_in_file: roads.len(),
        junctions_in_file: junctions,
        ..Default::default()
    };
    let mut note = |n: &str| {
        if !stats.notes.iter().any(|m| m == n) {
            stats.notes.push(n.to_string());
        }
    };
    let junction_of: BTreeMap<&str, &str> = roads
        .iter()
        .filter(|r| r.junction != "-1")
        .map(|r| (r.id.as_str(), r.junction.as_str()))
        .collect();
    let lht = roads
        .iter()
        .filter_map(|r| r.left_hand)
        .next()
        .unwrap_or(file_lht);

    // Each kept road: its lane sections as (start, end, profile, points).
    struct Piece {
        start: f64,
        end: f64,
        profile: Profile,
        pts: Vec<(f64, f64)>,
    }
    let mut kept: Vec<(&Road, Vec<Piece>)> = Vec::new();
    for road in &roads {
        if road.junction != "-1" {
            stats.connecting_roads += 1;
            continue;
        }
        let line = reference_line(road);
        if line.len() < 2 {
            stats.skipped_roads += 1;
            continue;
        }
        let length = road.length.max(line.last().map_or(0.0, |p| p.0));
        // Section boundaries, merging sections shorter than 5 m into the one before.
        let mut secs: Vec<&Section> = road.sections.iter().collect();
        secs.sort_by(|a, b| a.s.total_cmp(&b.s));
        let mut bounds: Vec<(f64, f64, &Section)> = Vec::new();
        for (i, sec) in secs.iter().enumerate() {
            let end = secs.get(i + 1).map_or(length, |n| n.s);
            match bounds.last_mut() {
                Some(last) if end - sec.s < 5.0 => last.1 = end,
                _ => bounds.push((sec.s, end, sec)),
            }
        }
        if bounds.len() > 1 && bounds[0].1 - bounds[0].0 < 5.0 {
            let first = bounds.remove(0);
            bounds[0].0 = first.0;
        }
        // Each section's profile, then its points moved onto its divider.
        let profiles: Vec<(f64, f64, Profile)> = bounds
            .into_iter()
            .map(|(start, end, sec)| (start, end, profile(sec, end - start)))
            .filter(|(_, _, p)| p.left + p.right > 0)
            .collect();
        // Where the divider jumps between touching sections (a one-way road
        // losing a lane re-centres), blend it over up to 10 m each side so the
        // line stays continuous; the network's taper then lines the lanes up.
        let divider_at = |k: usize, s: f64| {
            let (start, end, ref p) = profiles[k];
            let blend = 10.0f64.min((end - start) / 4.0);
            let mut d = p.divider_m;
            let touching = |o: usize| {
                (profiles[o].1 - start).abs() < 1e-6 || (profiles[o].0 - end).abs() < 1e-6
            };
            if k > 0 && touching(k - 1) && s - start < blend {
                let mid = (p.divider_m + profiles[k - 1].2.divider_m) / 2.0;
                d = mid + (p.divider_m - mid) * ((s - start) / blend);
            }
            if k + 1 < profiles.len() && touching(k + 1) && end - s < blend {
                let mid = (p.divider_m + profiles[k + 1].2.divider_m) / 2.0;
                d = mid + (p.divider_m - mid) * ((end - s) / blend);
            }
            d
        };
        let at = |s: f64| {
            let i = line.partition_point(|p| p.0 < s).clamp(1, line.len() - 1);
            let (a, b) = (line[i - 1], line[i]);
            let u = if b.0 > a.0 {
                ((s - a.0) / (b.0 - a.0)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (
                s,
                a.1 + (b.1 - a.1) * u,
                a.2 + (b.2 - a.2) * u,
                a.3 + (b.3 - a.3) * u,
            )
        };
        let mut pieces = Vec::new();
        for (k, &(start, end, _)) in profiles.iter().enumerate() {
            let shift = |p: &(f64, f64, f64, f64)| {
                let d = divider_at(k, p.0);
                (p.1 - d * p.3.sin(), p.2 + d * p.3.cos())
            };
            let mut pts: Vec<(f64, f64)> = vec![shift(&at(start))];
            pts.extend(
                line.iter()
                    .filter(|p| p.0 > start + 1e-6 && p.0 < end - 1e-6)
                    .map(shift),
            );
            pts.push(shift(&at(end)));
            pieces.push((start, end, pts));
        }
        let pieces: Vec<Piece> = pieces
            .into_iter()
            .zip(profiles)
            .map(|((start, end, pts), (_, _, profile))| Piece {
                start,
                end,
                profile,
                pts,
            })
            .collect();
        if pieces.is_empty() {
            stats.skipped_roads += 1;
            continue;
        }
        let gaps = pieces
            .windows(2)
            .any(|w| (w[0].end - w[1].start).abs() > 1e-6);
        if gaps {
            note("some roads lose their driving lanes part-way; those parts were left out");
        }
        kept.push((road, pieces));
    }
    if kept.is_empty() {
        return Err("no roads with driving lanes in this file".into());
    }

    // Join road ends into nodes: shared ends of linked roads, one node per junction.
    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    fn find(parent: &mut BTreeMap<String, String>, k: &str) -> String {
        let p = parent
            .entry(k.to_string())
            .or_insert_with(|| k.to_string())
            .clone();
        if p == k {
            return p;
        }
        let root = find(parent, &p);
        parent.insert(k.to_string(), root.clone());
        root
    }
    let union = |parent: &mut BTreeMap<String, String>, a: &str, b: &str| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            // Junction keys stay roots so a set knows it is a junction.
            let (keep, other) = if rb.starts_with('J') {
                (rb, ra)
            } else {
                (ra, rb)
            };
            parent.insert(other, keep);
        }
    };
    let end_key = |id: &str, at_start: bool| format!("R{id}:{}", if at_start { 0 } else { 1 });
    let kept_ids: Vec<&str> = kept.iter().map(|(r, _)| r.id.as_str()).collect();
    for (road, pieces) in &kept {
        // Only ends that are real road ends (not cut off by a missing section).
        let ends = [
            (true, road.pred.as_ref(), pieces[0].start < 1e-6),
            (
                false,
                road.succ.as_ref(),
                pieces.last().is_some_and(|p| p.end >= road.length - 1e-3),
            ),
        ];
        for (at_start, link, real) in ends {
            let me = end_key(&road.id, at_start);
            find(&mut parent, &me);
            let Some(link) = link.filter(|_| real) else {
                continue;
            };
            if link.junction {
                union(&mut parent, &me, &format!("J{}", link.id));
            } else if let Some(j) = junction_of.get(link.id.as_str()) {
                union(&mut parent, &me, &format!("J{j}"));
            } else if kept_ids.contains(&link.id.as_str()) {
                union(&mut parent, &me, &end_key(&link.id, link.at_start));
            }
        }
    }
    // Node positions. Where linked roads simply meet, the mean of their ends.
    // At a junction, the point closest to every arm's centre line (where the
    // arms would cross), so roads run straight into it.
    // Per node: (end point, direction, driving lanes) of each road end.
    type Arm = ((f64, f64), (f64, f64), u8);
    let mut ends: BTreeMap<String, Vec<Arm>> = BTreeMap::new();
    for (road, pieces) in &kept {
        let first = &pieces[0].pts;
        let last = &pieces.last().unwrap().pts;
        let dir = |a: (f64, f64), b: (f64, f64)| {
            let l = (b.0 - a.0).hypot(b.1 - a.1).max(1e-9);
            ((b.0 - a.0) / l, (b.1 - a.1) / l)
        };
        let n = last.len();
        let lanes = |p: &Piece| p.profile.left + p.profile.right;
        for (at_start, p, d, k) in [
            (true, first[0], dir(first[0], first[1]), lanes(&pieces[0])),
            (
                false,
                last[n - 1],
                dir(last[n - 2], last[n - 1]),
                lanes(pieces.last().unwrap()),
            ),
        ] {
            let root = find(&mut parent, &end_key(&road.id, at_start));
            ends.entry(root).or_default().push((p, d, k));
        }
    }
    let node_position = |root: &str, arms: &[Arm]| {
        let k = arms.len() as f64;
        let mean = (
            arms.iter().map(|a| a.0.0).sum::<f64>() / k,
            arms.iter().map(|a| a.0.1).sum::<f64>() / k,
        );
        if !root.starts_with('J') {
            return mean;
        }
        // Least squares: sum over arms of (I - d dᵀ) p = (I - d dᵀ) a.
        let (mut m, mut r) = ([0.0f64; 3], [0.0f64; 2]);
        for ((ax, ay), (dx, dy), _) in arms {
            let (xx, xy, yy) = (1.0 - dx * dx, -dx * dy, 1.0 - dy * dy);
            m[0] += xx;
            m[1] += xy;
            m[2] += yy;
            r[0] += xx * ax + xy * ay;
            r[1] += xy * ax + yy * ay;
        }
        let det = m[0] * m[2] - m[1] * m[1];
        // Arms (nearly) parallel, as where a slip road merges: the main road
        // (the widest arm) runs straight through, the others join it there.
        let widest = arms.iter().max_by_key(|a| a.2).map_or(mean, |a| a.0);
        if det.abs() < 1e-3 * k * k {
            return widest;
        }
        let p = (
            (m[2] * r[0] - m[1] * r[1]) / det,
            (m[0] * r[1] - m[1] * r[0]) / det,
        );
        // Trust it only if it is near the arms' ends (not far off along a shallow angle).
        let spread = arms
            .iter()
            .map(|a| (a.0.0 - mean.0).hypot(a.0.1 - mean.1))
            .fold(0.0, f64::max);
        if (p.0 - mean.0).hypot(p.1 - mean.1) > spread.max(5.0) {
            widest
        } else {
            p
        }
    };
    // Recentre far-away coordinates so f32 keeps centimetre precision.
    let all = kept
        .iter()
        .flat_map(|(_, ps)| ps.iter().flat_map(|p| p.pts.iter()));
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in all {
        (x0, y0, x1, y1) = (x0.min(p.0), y0.min(p.1), x1.max(p.0), y1.max(p.1));
    }
    let centre = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let shift = if centre.0.hypot(centre.1) > 10_000.0 {
        centre
    } else {
        (0.0, 0.0)
    };
    stats.shift = shift;
    stats.extent_m = ((x1 - x0) as f32, (y1 - y0) as f32);
    let local = |p: (f64, f64)| ((p.0 - shift.0) as f32, (p.1 - shift.1) as f32);

    let mut node_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut nodes = Vec::new();
    for (root, arms) in &ends {
        let id = if let Some(j) = root.strip_prefix('J') {
            format!("j{j}")
        } else {
            format!("n{}", node_ids.len())
        };
        node_ids.insert(root.clone(), id.clone());
        nodes.push(NodeDef {
            id,
            position: local(node_position(root, arms)),
            control: Default::default(),
        });
    }
    let mut out_roads = Vec::new();
    for (road, pieces) in &kept {
        let start_node = node_ids[&find(&mut parent, &end_key(&road.id, true))].clone();
        let end_node = node_ids[&find(&mut parent, &end_key(&road.id, false))].clone();
        let many = pieces.len() > 1;
        let mut from = start_node.clone();
        for (k, piece) in pieces.iter().enumerate() {
            let last = k == pieces.len() - 1;
            let to = if last {
                end_node.clone()
            } else {
                // A node where the lane layout changes.
                let id = format!("r{}_s{}", road.id, k + 1);
                nodes.push(NodeDef {
                    id: id.clone(),
                    position: local(*piece.pts.last().unwrap()),
                    control: Default::default(),
                });
                id
            };
            let pos = |id: &str| nodes.iter().find(|n| n.id == id).map(|n| n.position);
            // Interior points: the sampled line, keeping its true ends when the
            // node sits elsewhere (a junction's centre), but not points within
            // a metre of a node, which would leave a kink.
            let near = |a: (f32, f32), b: Option<(f32, f32)>| {
                b.is_some_and(|b| (a.0 - b.0).hypot(a.1 - b.1) < 1.0)
            };
            let (pf, pt) = (pos(&from), pos(&to));
            let inner: Vec<(f32, f32)> = piece
                .pts
                .iter()
                .map(|p| local(*p))
                .filter(|&p| !near(p, pf) && !near(p, pt))
                .collect();
            let (forward, backward) = if lht {
                (piece.profile.left, piece.profile.right)
            } else {
                (piece.profile.right, piece.profile.left)
            };
            let id = if many {
                format!("r{}_{k}", road.id)
            } else {
                format!("r{}", road.id)
            };
            let make = |id: String, from: String, to: String, points: Vec<(f32, f32)>| RoadDef {
                id,
                name: road.name.clone(),
                from,
                to,
                shape: RoadShape::Polyline { points },
                lanes_forward: forward,
                lanes_backward: backward,
                lane_width_m: piece.profile.lane_width_m.clamp(2.0, 6.0),
                speed_limit_kmh: road.speed_kmh.unwrap_or(50.0).clamp(5.0, 200.0),
                sidewalk_m: piece.profile.sidewalk_m.min(5.0),
            };
            if from == to {
                // A loop back to the same node: split it in two.
                let mid = inner.len() / 2;
                let mid_id = format!("{id}_mid");
                nodes.push(NodeDef {
                    id: mid_id.clone(),
                    position: inner.get(mid).copied().unwrap_or_default(),
                    control: Default::default(),
                });
                out_roads.push(make(
                    format!("{id}a"),
                    from.clone(),
                    mid_id.clone(),
                    inner[..mid].to_vec(),
                ));
                out_roads.push(make(
                    format!("{id}b"),
                    mid_id,
                    to.clone(),
                    inner[(mid + 1).min(inner.len())..].to_vec(),
                ));
            } else {
                out_roads.push(make(id, from.clone(), to.clone(), inner));
            }
            from = to;
        }
    }
    if !roads
        .iter()
        .all(|r| r.geometry.iter().all(|g| g.length >= 0.0))
    {
        note("negative geometry lengths were treated as zero");
    }
    note("elevation, superelevation and road marks are ignored (the engine is planar)");
    stats.roads = out_roads.len();
    let net = RoadNetworkDef {
        stops: Vec::new(),
        name: name.to_string(),
        drive_side: if lht {
            DriveSide::Left
        } else {
            DriveSide::Right
        },
        nodes,
        roads: out_roads,
    };
    Ok((net, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::RoadNetwork;

    /// A crossing: road 1 (west→junction, arc then line), road 2 (junction→east,
    /// spiral, with a lane section that adds a lane), road 3 (one-way, north,
    /// paramPoly3), plus a connecting road inside junction 100.
    const XODR: &str = include_str!("../tests/data/crossing.xodr");

    #[test]
    fn imports_roads_sections_and_junctions() {
        let (net, stats) = import(XODR, "t").unwrap();
        assert_eq!(
            stats.connecting_roads, 1,
            "the junction's own road is replaced"
        );
        assert_eq!(stats.junctions_in_file, 1);
        assert_eq!(net.drive_side, DriveSide::Right);
        let ids: Vec<&str> = net.roads.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            ["r1", "r2_0", "r2_1", "r3"],
            "road 2 splits where a lane is added"
        );
        let r1 = &net.roads[0];
        assert_eq!(r1.name.as_deref(), Some("West Street"));
        assert!((r1.speed_limit_kmh - 48.27).abs() < 0.1, "30 mph");
        assert_eq!(
            (r1.lanes_forward, r1.lanes_backward, r1.sidewalk_m),
            (1, 1, 2.0)
        );
        assert_eq!(
            (net.roads[1].lanes_forward, net.roads[2].lanes_forward),
            (1, 2)
        );
        let r3 = &net.roads[3];
        assert_eq!((r3.lanes_forward, r3.lanes_backward), (2, 0));
        // All three roads meet at the junction node, at the centre of their ends.
        assert_eq!(net.roads[0].to, "j100");
        assert_eq!(net.roads[1].from, "j100");
        assert_eq!(r3.from, "j100");
        let j = net.node("j100").unwrap().position;
        assert!(
            j.0.abs() < 10.0 && j.1.abs() < 10.0,
            "junction centre {j:?}"
        );
        // A one-way road's line moves to the middle of its two lanes: 3.5 m right of the reference.
        let RoadShape::Polyline { points } = &r3.shape else {
            panic!()
        };
        assert!(
            (points[0].0 - 3.5).abs() < 0.05,
            "one-way centred: {:?}",
            points[0]
        );
        // The result builds and routes from the west street to the north road.
        let built = RoadNetwork::build(&net);
        let start = built.lane("r1:f0").unwrap();
        assert!(built.route(start, "r3").is_some());
        assert!(built.route(start, "r2_1").is_some());
    }

    #[test]
    fn samples_curves_exactly() {
        // A quarter circle of radius 50 ends at (50, 50) heading north.
        let g = Geometry {
            s: 0.0,
            x: 0.0,
            y: 0.0,
            hdg: 0.0,
            length: std::f64::consts::FRAC_PI_2 * 50.0,
            shape: Shape::Arc(0.02),
        };
        let mut pts = Vec::new();
        sample(&g, &mut pts);
        let end = pts.last().unwrap();
        assert!(
            (end.1 - 50.0).abs() < 1e-6 && (end.2 - 50.0).abs() < 1e-6,
            "{end:?}"
        );
        // A clothoid from straight to curvature 0.02 over 100 m turns by 1 rad.
        let g = Geometry {
            shape: Shape::Spiral(0.0, 0.02),
            length: 100.0,
            ..g
        };
        let mut pts = Vec::new();
        sample(&g, &mut pts);
        let end = pts.last().unwrap();
        assert!((end.3 - 1.0).abs() < 1e-6);
        // Its end, by the Fresnel integrals: about (90.45, 31.03).
        assert!(
            (end.1 - 90.45).abs() < 0.05 && (end.2 - 31.03).abs() < 0.05,
            "{end:?}"
        );
    }

    #[test]
    fn rejects_other_xml() {
        assert!(import("<osm></osm>", "x").is_err());
        assert!(import("<OpenDRIVE><header/></OpenDRIVE>", "x").is_err());
    }
}
