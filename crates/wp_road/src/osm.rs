//! OpenStreetMap import: OSM XML → [`RoadNetworkDef`].
//!
//! Keeps drivable `highway=*` ways, reads `lanes`, `lanes:forward/backward`,
//! `oneway`, `maxspeed` and `name`, projects latitude/longitude to metres
//! around the data's centre, and splits ways into roads wherever they meet.
//! Map data © OpenStreetMap contributors, available under the ODbL.

use crate::def::{DriveSide, NodeDef, RoadDef, RoadNetworkDef, RoadShape};
use quick_xml::events::{BytesStart, Event};
use std::collections::{BTreeMap, BTreeSet};

/// What an import found, for the report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportStats {
    pub osm_nodes: usize,
    pub osm_ways: usize,
    pub drivable_ways: usize,
    /// Whether ways were clipped to the file's `<bounds>`.
    pub clipped: bool,
    pub roads: usize,
    pub junctions: usize,
    /// Size of the area covered, m (east-west, north-south).
    pub extent_m: (f32, f32),
    /// Latitude and longitude of the local origin (0, 0).
    pub origin: (f64, f64),
}

#[derive(Clone)]
struct Way {
    id: String,
    nodes: Vec<String>,
    tags: BTreeMap<String, String>,
}

/// A building outline from the map (`building=*` closed ways), in the same
/// local metres as the roads.
#[derive(Clone, Debug, PartialEq)]
pub struct OsmBuilding {
    /// The OSM way id.
    pub id: String,
    /// Outline without the closing point.
    pub footprint: Vec<(f32, f32)>,
    /// From `height`, else `building:levels` × 3 m + 0.5 m, else a typical
    /// height for its kind (varied a little by id, so streets are not flat).
    pub height_m: f32,
    /// The `building` tag: house, apartments, commercial, yes, ...
    pub kind: String,
}

/// A tree from the map (`natural=tree` nodes).
#[derive(Clone, Debug, PartialEq)]
pub struct OsmTree {
    pub position: (f32, f32),
    /// From `height`, else 8 m.
    pub height_m: f32,
    /// `leaf_type=needleleaved` (a conifer) or a palm (`genus`/`taxon` names one).
    pub needles: bool,
    pub palm: bool,
}

/// What `import_with_buildings` found besides roads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OsmScenery {
    pub buildings: Vec<OsmBuilding>,
    pub trees: Vec<OsmTree>,
}

/// "12", "12 m", "12.5m" → metres (feet and other units are not read).
fn parse_metres(s: &str) -> Option<f32> {
    let t = s.trim();
    if t.contains('\'') || t.contains("ft") {
        return None;
    }
    let num: String = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().ok().filter(|v: &f32| *v > 0.0)
}

/// Typical height of a building of this kind, m; `vary` (0–1) picks within the range.
fn typical_height(kind: &str, vary: f32) -> f32 {
    let (lo, hi) = match kind {
        "house" | "detached" | "semidetached_house" | "terrace" | "bungalow" | "cabin" | "farm"
        | "hut" => (4.0, 8.0),
        "garage" | "garages" | "shed" | "kiosk" | "service" | "toilets" => (2.5, 3.5),
        "apartments" | "residential" | "dormitory" | "hotel" => (12.0, 30.0),
        "commercial" | "retail" | "office" | "bank" | "government" => (9.0, 30.0),
        "industrial" | "warehouse" | "factory" | "manufacture" | "hangar" => (6.0, 12.0),
        "school" | "university" | "college" | "hospital" | "public" | "civic" => (8.0, 18.0),
        "mosque" | "church" | "temple" | "cathedral" | "chapel" => (8.0, 16.0),
        _ => (6.0, 16.0),
    };
    lo + (hi - lo) * vary.clamp(0.0, 1.0)
}

/// Lane count per direction, lane width and default speed for a highway class.
fn class_defaults(highway: &str) -> Option<(u8, f32, f32)> {
    Some(match highway {
        "motorway" => (2, 3.75, 110.0),
        "trunk" => (2, 3.5, 90.0),
        "primary" => (1, 3.5, 60.0),
        "secondary" => (1, 3.5, 50.0),
        "tertiary" => (1, 3.25, 50.0),
        "motorway_link" | "trunk_link" | "primary_link" | "secondary_link" | "tertiary_link" => {
            (1, 3.5, 40.0)
        }
        "unclassified" | "residential" => (1, 3.0, 30.0),
        "living_street" => (1, 3.0, 20.0),
        "service" => (1, 2.75, 20.0),
        _ => return None, // footways, cycleways, paths, tracks, ...
    })
}

/// "50", "50 km/h", "30 mph" → km/h.
fn parse_speed(s: &str) -> Option<f32> {
    let t = s.trim();
    let num: String = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let v: f32 = num.parse().ok()?;
    Some(if t.contains("mph") { v * 1.609 } else { v })
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

/// Convert OSM XML to a road network. `name` names the network; `lang`
/// (e.g. "en") prefers `name:<lang>` tags for road names when present.
pub fn import(
    xml: &str,
    name: &str,
    drive_side: DriveSide,
    lang: Option<&str>,
) -> Result<(RoadNetworkDef, ImportStats), String> {
    import_with_buildings(xml, name, drive_side, lang).map(|(net, _, stats)| (net, stats))
}

fn is_palm(tags: &BTreeMap<String, String>) -> bool {
    ["genus", "taxon", "species", "genus:en", "species:en"]
        .iter()
        .filter_map(|k| tags.get(*k))
        .any(|v| {
            let v = v.to_lowercase();
            [
                "palm",
                "cocos",
                "areca",
                "phoenix",
                "roystonea",
                "washingtonia",
                "borassus",
            ]
            .iter()
            .any(|p| v.contains(p))
        })
}

/// [`import`], plus the buildings inside the exported area (whole outlines
/// only; outlines cut by the edge are left out, as are multipolygon
/// relations) and its `natural=tree` trees.
pub fn import_with_buildings(
    xml: &str,
    name: &str,
    drive_side: DriveSide,
    lang: Option<&str>,
) -> Result<(RoadNetworkDef, OsmScenery, ImportStats), String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut coords: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    let mut ways: Vec<Way> = Vec::new();
    let mut current: Option<Way> = None;
    // (min lat, min lon, max lat, max lon) of the exported area, if given.
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    // A node with tags being read (only `<node>…</node>` elements have them).
    let mut tagged: Option<(String, BTreeMap<String, String>)> = None;
    let mut tree_nodes: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("invalid OSM XML at byte {}: {e}", reader.buffer_position()))?;
        let opens = matches!(event, Event::Start(_));
        match event {
            Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                "node" => {
                    if let (Some(id), Some(lat), Some(lon)) =
                        (attr(&e, "id"), attr(&e, "lat"), attr(&e, "lon"))
                        && let (Ok(lat), Ok(lon)) = (lat.parse::<f64>(), lon.parse::<f64>())
                    {
                        if opens {
                            tagged = Some((id.clone(), BTreeMap::new()));
                        }
                        coords.insert(id, (lat, lon));
                    }
                }
                "bounds" => {
                    let f = |k: &str| attr(&e, k).and_then(|v| v.parse::<f64>().ok());
                    if let (Some(a), Some(b), Some(c), Some(d)) =
                        (f("minlat"), f("minlon"), f("maxlat"), f("maxlon"))
                    {
                        bounds = Some((a, b, c, d));
                    }
                }
                "way" => {
                    current = Some(Way {
                        id: attr(&e, "id").unwrap_or_default(),
                        nodes: Vec::new(),
                        tags: BTreeMap::new(),
                    });
                }
                "nd" => {
                    if let (Some(w), Some(r)) = (current.as_mut(), attr(&e, "ref")) {
                        w.nodes.push(r);
                    }
                }
                "tag" => {
                    if let (Some(k), Some(v)) = (attr(&e, "k"), attr(&e, "v")) {
                        if let Some(w) = current.as_mut() {
                            w.tags.insert(k, v);
                        } else if let Some((_, tags)) = tagged.as_mut() {
                            tags.insert(k, v);
                        }
                    }
                }
                _ => {}
            },
            Event::End(e) if e.name().as_ref() == "node" => {
                if let Some((id, tags)) = tagged.take()
                    && tags.get("natural").is_some_and(|n| n == "tree")
                {
                    tree_nodes.push((id, tags));
                }
            }
            Event::End(e) if e.name().as_ref() == "way" => {
                if let Some(w) = current.take() {
                    ways.push(w);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let osm_ways = ways.len();
    // A node is usable if we have its position and it lies inside the
    // exported area (OSM exports whole ways, which run past the edges).
    let usable = |n: &String| {
        coords.get(n).is_some_and(|&(lat, lon)| {
            bounds.is_none_or(|(a, b, c, d)| lat >= a && lat <= c && lon >= b && lon <= d)
        })
    };
    let building_ways: Vec<Way> = ways
        .iter()
        .filter(|w| {
            w.tags.get("building").is_some_and(|b| {
                !matches!(
                    b.as_str(),
                    "no" | "roof" | "carport" | "ruins" | "collapsed"
                )
            }) && w.nodes.len() >= 4
                && w.nodes.first() == w.nodes.last()
                && w.nodes.iter().all(&usable)
        })
        .cloned()
        .collect();
    let mut drivable: Vec<Way> = Vec::new();
    for w in ways {
        let keep = w
            .tags
            .get("highway")
            .is_some_and(|h| class_defaults(h).is_some())
            && w.tags.get("area").is_none_or(|a| a != "yes");
        if !keep {
            continue;
        }
        // Split the way into runs of usable nodes.
        let mut runs: Vec<Vec<String>> = vec![Vec::new()];
        for n in &w.nodes {
            if usable(n) {
                runs.last_mut().expect("never empty").push(n.clone());
            } else if !runs.last().expect("never empty").is_empty() {
                runs.push(Vec::new());
            }
        }
        runs.retain(|r| r.len() >= 2);
        let many = runs.len() > 1;
        for (k, nodes) in runs.into_iter().enumerate() {
            drivable.push(Way {
                id: if many {
                    format!("{}.{k}", w.id)
                } else {
                    w.id.clone()
                },
                nodes,
                tags: w.tags.clone(),
            });
        }
    }
    if drivable.is_empty() {
        return Err("no drivable roads (highway=*) in this data".into());
    }

    // Local projection around the centre of the nodes the roads use.
    let used: BTreeSet<&String> = drivable.iter().flat_map(|w| &w.nodes).collect();
    let (mut lat_min, mut lat_max, mut lon_min, mut lon_max) =
        (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for id in &used {
        let (lat, lon) = coords[*id];
        lat_min = lat_min.min(lat);
        lat_max = lat_max.max(lat);
        lon_min = lon_min.min(lon);
        lon_max = lon_max.max(lon);
    }
    let (lat0, lon0) = ((lat_min + lat_max) / 2.0, (lon_min + lon_max) / 2.0);
    let m_per_deg_lat = 111_132.0;
    let m_per_deg_lon = 111_320.0 * lat0.to_radians().cos();
    let project = |id: &str| {
        let (lat, lon) = coords[id];
        (
            ((lon - lon0) * m_per_deg_lon) as f32,
            ((lat - lat0) * m_per_deg_lat) as f32,
        )
    };

    // Split ways where they share nodes with other ways (junctions).
    let mut use_count: BTreeMap<&String, u32> = BTreeMap::new();
    for w in &drivable {
        for (i, n) in w.nodes.iter().enumerate() {
            let ends = if i == 0 || i == w.nodes.len() - 1 {
                2
            } else {
                1
            };
            *use_count.entry(n).or_default() += ends;
        }
    }
    let is_split = |n: &String| use_count.get(n).copied().unwrap_or(0) >= 2;
    let mut roads = Vec::new();
    let mut graph_nodes: BTreeSet<String> = BTreeSet::new();
    for w in &drivable {
        let highway = w.tags["highway"].as_str();
        let (default_lanes, lane_w, default_speed) =
            class_defaults(highway).expect("filtered above");
        let oneway = match w.tags.get("oneway").map(String::as_str) {
            Some("yes" | "true" | "1") => 1,
            Some("-1" | "reverse") => -1,
            _ if highway == "motorway"
                || w.tags.get("junction").is_some_and(|j| j == "roundabout") =>
            {
                1
            }
            _ => 0,
        };
        let total: Option<u8> = w.tags.get("lanes").and_then(|l| l.parse().ok());
        let fwd_tag: Option<u8> = w.tags.get("lanes:forward").and_then(|l| l.parse().ok());
        let bwd_tag: Option<u8> = w.tags.get("lanes:backward").and_then(|l| l.parse().ok());
        let (mut fwd, mut bwd) = match (oneway, total) {
            (0, Some(t)) => (
                fwd_tag.unwrap_or(t.div_ceil(2)).max(1),
                bwd_tag.unwrap_or(t / 2).max(1),
            ),
            (0, None) => (default_lanes, default_lanes),
            (_, t) => (t.unwrap_or(default_lanes).max(1), 0),
        };
        let mut nodes = w.nodes.clone();
        if oneway == -1 {
            nodes.reverse();
        }
        if oneway != 0 {
            bwd = 0;
            fwd = fwd.max(1);
        }
        let speed = w
            .tags
            .get("maxspeed")
            .and_then(|s| parse_speed(s))
            .unwrap_or(default_speed)
            .clamp(5.0, 200.0);
        // Cut the way at junction nodes.
        let mut start = 0;
        let mut part = 0;
        for i in 1..nodes.len() {
            if i == nodes.len() - 1 || is_split(&nodes[i]) {
                let seg = &nodes[start..=i];
                let a = project(&seg[0]);
                let b = project(&seg[seg.len() - 1]);
                let chord = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
                if chord > 1.0 && seg[0] != seg[seg.len() - 1] {
                    let inner: Vec<(f32, f32)> =
                        seg[1..seg.len() - 1].iter().map(|n| project(n)).collect();
                    graph_nodes.insert(seg[0].clone());
                    graph_nodes.insert(seg[seg.len() - 1].clone());
                    roads.push(RoadDef {
                        id: format!("w{}_{part}", w.id),
                        name: lang
                            .and_then(|l| w.tags.get(&format!("name:{l}")))
                            .or(w.tags.get("name"))
                            .cloned(),
                        from: format!("n{}", seg[0]),
                        to: format!("n{}", seg[seg.len() - 1]),
                        shape: if inner.is_empty() {
                            RoadShape::Straight
                        } else {
                            RoadShape::Polyline { points: inner }
                        },
                        lanes_forward: fwd,
                        lanes_backward: bwd,
                        lane_width_m: lane_w,
                        speed_limit_kmh: speed,
                        sidewalk_m: 0.0,
                    });
                    part += 1;
                }
                start = i;
            }
        }
    }
    let nodes: Vec<NodeDef> = graph_nodes
        .iter()
        .map(|id| NodeDef {
            id: format!("n{id}"),
            position: project(id),
            control: Default::default(),
        })
        .collect();
    let mut degree: BTreeMap<&str, u32> = BTreeMap::new();
    for r in &roads {
        *degree.entry(r.from.as_str()).or_default() += 1;
        *degree.entry(r.to.as_str()).or_default() += 1;
    }
    let buildings: Vec<OsmBuilding> = building_ways
        .iter()
        .map(|w| {
            let kind = w.tags["building"].clone();
            let vary = (w
                .id
                .bytes()
                .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
                % 1000) as f32
                / 999.0;
            let height_m = w
                .tags
                .get("height")
                .and_then(|h| parse_metres(h))
                .or_else(|| {
                    w.tags
                        .get("building:levels")
                        .and_then(|l| parse_metres(l))
                        .map(|l| l * 3.0 + 0.5)
                })
                .unwrap_or_else(|| typical_height(&kind, vary))
                .clamp(2.5, 600.0);
            OsmBuilding {
                id: w.id.clone(),
                footprint: w.nodes[..w.nodes.len() - 1]
                    .iter()
                    .map(|n| project(n))
                    .collect(),
                height_m,
                kind,
            }
        })
        .collect();
    let inside = |id: &String| usable(id);
    let trees: Vec<OsmTree> = tree_nodes
        .iter()
        .filter(|(id, _)| inside(id))
        .map(|(id, tags)| OsmTree {
            position: project(id),
            height_m: tags
                .get("height")
                .and_then(|h| parse_metres(h))
                .unwrap_or(8.0)
                .clamp(2.0, 60.0),
            needles: tags.get("leaf_type").is_some_and(|l| l == "needleleaved"),
            palm: is_palm(tags),
        })
        .collect();
    let stats = ImportStats {
        osm_nodes: coords.len(),
        osm_ways,
        drivable_ways: drivable.len(),
        clipped: bounds.is_some(),
        roads: roads.len(),
        junctions: degree.values().filter(|d| **d >= 3).count(),
        extent_m: (
            ((lon_max - lon_min) * m_per_deg_lon) as f32,
            ((lat_max - lat_min) * m_per_deg_lat) as f32,
        ),
        origin: (lat0, lon0),
    };
    Ok((
        RoadNetworkDef {
            stops: Vec::new(),
            name: name.to_string(),
            drive_side,
            nodes,
            roads,
        },
        OsmScenery { buildings, trees },
        stats,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::RoadNetwork;

    /// A T-junction: a two-way street "Main" crossed by one-way "Side", plus
    /// a footway that must be ignored.
    const OSM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<osm version="0.6">
  <node id="1" lat="23.8100" lon="90.4100"/>
  <node id="2" lat="23.8100" lon="90.4110"/>
  <node id="3" lat="23.8100" lon="90.4120"/>
  <node id="4" lat="23.8110" lon="90.4110"/>
  <node id="5" lat="23.8090" lon="90.4100"/>
  <way id="10"><nd ref="1"/><nd ref="2"/><nd ref="3"/>
    <tag k="highway" v="primary"/><tag k="name" v="প্রধান সড়ক"/><tag k="name:en" v="Main &amp; Co"/><tag k="lanes" v="4"/><tag k="maxspeed" v="40"/></way>
  <way id="11"><nd ref="4"/><nd ref="2"/>
    <tag k="highway" v="residential"/><tag k="oneway" v="yes"/></way>
  <way id="12"><nd ref="1"/><nd ref="5"/><tag k="highway" v="footway"/></way>
</osm>"#;

    #[test]
    fn imports_roads_and_splits_at_junctions() {
        let (net, stats) = import(OSM, "t", DriveSide::Left, Some("en")).unwrap();
        assert_eq!(stats.drivable_ways, 2, "the footway is skipped");
        // Main is split at node 2 into two roads; Side is one road.
        assert_eq!(
            net.roads.len(),
            3,
            "{:?}",
            net.roads.iter().map(|r| &r.id).collect::<Vec<_>>()
        );
        let main = net.roads.iter().find(|r| r.id == "w10_0").unwrap();
        assert_eq!(main.name.as_deref(), Some("Main & Co"));
        assert_eq!((main.lanes_forward, main.lanes_backward), (2, 2));
        assert_eq!(main.speed_limit_kmh, 40.0);
        let side = net.roads.iter().find(|r| r.id == "w11_0").unwrap();
        assert_eq!((side.lanes_forward, side.lanes_backward), (1, 0), "one-way");
        // About 102 m between nodes 1 and 2 at this latitude.
        let a = net.node("n1").unwrap().position;
        let b = net.node("n2").unwrap().position;
        let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        assert!((d - 102.0).abs() < 2.0, "projected distance {d}");
        assert_eq!(stats.junctions, 1);
        // The result builds into a routable network.
        let built = RoadNetwork::build(&net);
        let start = built.lane("w11_0:f0").unwrap();
        assert!(built.route(start, "w10_1").is_some() || built.route(start, "w10_0").is_some());
    }

    #[test]
    fn clips_ways_to_bounds() {
        // Node 3 lies east of the bounds: Main ends at node 2.
        let xml = OSM.replace(
            "<osm version=\"0.6\">",
            "<osm version=\"0.6\"><bounds minlat=\"23.80\" minlon=\"90.40\" maxlat=\"23.82\" maxlon=\"90.4115\"/>",
        );
        let (net, stats) = import(&xml, "t", DriveSide::Left, None).unwrap();
        assert!(stats.clipped);
        assert!(
            net.node("n3").is_none(),
            "node outside the bounds is dropped"
        );
        assert!(net.roads.iter().all(|r| r.to != "n3" && r.from != "n3"));
    }

    #[test]
    fn imports_buildings_with_heights() {
        let xml = OSM.replace(
            "</osm>",
            r#"<node id="20" lat="23.8102" lon="90.4102"/><node id="21" lat="23.8102" lon="90.4104"/>
  <node id="22" lat="23.8104" lon="90.4104"/><node id="23" lat="23.8104" lon="90.4102"/>
  <way id="30"><nd ref="20"/><nd ref="21"/><nd ref="22"/><nd ref="23"/><nd ref="20"/>
    <tag k="building" v="apartments"/><tag k="building:levels" v="6"/></way>
  <way id="31"><nd ref="20"/><nd ref="21"/><nd ref="22"/><nd ref="20"/>
    <tag k="building" v="yes"/><tag k="height" v="14 m"/></way>
  <way id="32"><nd ref="20"/><nd ref="21"/><nd ref="22"/><tag k="building" v="yes"/></way>
  <way id="33"><nd ref="20"/><nd ref="21"/><nd ref="22"/><nd ref="20"/><tag k="building" v="no"/></way>
  <node id="40" lat="23.8101" lon="90.4101"><tag k="natural" v="tree"/><tag k="genus" v="Cocos"/></node>
  <node id="41" lat="23.8101" lon="90.4103"><tag k="natural" v="tree"/><tag k="height" v="12"/></node>
  <node id="42" lat="23.8101" lon="90.4105"><tag k="amenity" v="bench"/></node>
</osm>"#,
        );
        let (_, sc, _) = import_with_buildings(&xml, "t", DriveSide::Left, None).unwrap();
        assert_eq!(sc.trees.len(), 2);
        assert!(sc.trees[0].palm && !sc.trees[1].palm);
        assert_eq!(sc.trees[1].height_m, 12.0);
        let b = sc.buildings;
        assert_eq!(b.len(), 2, "open and building=no ways are skipped");
        assert_eq!(b[0].footprint.len(), 4, "closing point dropped");
        assert_eq!(b[0].height_m, 18.5);
        assert_eq!(b[0].kind, "apartments");
        assert_eq!(b[1].height_m, 14.0);
        // About 20 m square.
        let (x0, x1) = (b[0].footprint[0].0, b[0].footprint[1].0);
        assert!(((x1 - x0) - 20.4).abs() < 0.5, "{}", x1 - x0);
    }

    #[test]
    fn speeds_parse_units() {
        assert_eq!(parse_speed("50"), Some(50.0));
        assert!((parse_speed("30 mph").unwrap() - 48.27).abs() < 0.1);
        assert_eq!(parse_speed("signals"), None);
    }

    #[test]
    fn rejects_data_without_roads() {
        assert!(
            import(
                r#"<osm><node id="1" lat="0" lon="0"/></osm>"#,
                "x",
                DriveSide::Right,
                None
            )
            .is_err()
        );
    }
}
