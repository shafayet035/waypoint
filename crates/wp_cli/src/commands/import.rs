//! `wp import osm`: turn OpenStreetMap data into a road network and a scene.

use super::*;
use waypoint::road::{DriveSide, LaneKind, RoadNetwork, osm};

pub fn import_osm(
    ctx: &Ctx,
    file: &Path,
    name: &str,
    drive_side: &str,
    lang: Option<&str>,
    force: bool,
) -> CmdResult {
    let side = match drive_side {
        "right" => DriveSide::Right,
        "left" => DriveSide::Left,
        other => {
            return Err(cli_error(
                format!("unknown drive side `{other}`"),
                "use --drive-side right (most countries) or left (UK, Japan, India, Bangladesh, ...)",
            ));
        }
    };
    if !ctx.project.join("game.toml").exists() {
        return Err(vec![
            Diagnostic::error(
                "WP-G001",
                format!("no game.toml in {}", ctx.project.display()),
            )
            .help("pass --project <dir>, or create one with `wp new <name>`"),
        ]);
    }
    let xml = std::fs::read_to_string(file).map_err(|e| {
        vec![
            Diagnostic::error("WP-D001", format!("cannot read {}: {e}", file.display()))
                .help("export an area from openstreetmap.org (Export → .osm) or the Overpass API"),
        ]
    })?;
    let (net, stats) = osm::import(&xml, name, side, lang).map_err(|e| {
        vec![
            Diagnostic::error("WP-I001", format!("cannot import {}: {e}", file.display()))
                .help("the file must be OSM XML (.osm) with highway=* ways and their nodes"),
        ]
    })?;

    // A spawn point halfway along the longest lane from which the most of
    // the map can be reached (edges of the data are often dead ends).
    let built = RoadNetwork::build(&net);
    let connected = built.largest_connected();
    let connected_roads: std::collections::BTreeSet<usize> = connected
        .iter()
        .filter_map(|&l| match built.lanes[l].kind {
            LaneKind::Road { road, .. } => Some(road),
            LaneKind::Connector { .. } => None,
        })
        .collect();
    let lane = connected
        .iter()
        .map(|&l| &built.lanes[l])
        .filter(|l| matches!(l.kind, LaneKind::Road { .. }))
        .max_by(|a, b| a.path.length().total_cmp(&b.path.length()))
        .ok_or_else(|| {
            vec![
                Diagnostic::error("WP-I001", "the imported roads have no lanes")
                    .help("pick a larger area with at least one drivable street"),
            ]
        })?;
    let (pos, dir) = lane.path.sample(lane.path.length() / 2.0);
    let heading = dir.y.atan2(dir.x).to_degrees();

    let roads_rel = format!("roads/{name}.road.ron");
    let scene_rel = format!("scenes/{name}.scene.ron");
    for rel in [&roads_rel, &scene_rel] {
        if ctx.project.join(rel).exists() && !force {
            return Err(cli_error(
                format!("{rel} already exists"),
                "pick another --name, or pass --force to overwrite",
            ));
        }
    }
    let source = file
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let header = format!(
        "// Imported from OpenStreetMap ({source}) by `wp import osm`.\n\
         // Map data © OpenStreetMap contributors, available under the Open Database\n\
         // License (ODbL): https://www.openstreetmap.org/copyright\n\
         // Origin (0, 0) = latitude {:.6}, longitude {:.6}; x = east, y = north, metres.\n",
        stats.origin.0, stats.origin.1
    );
    let roads_ron = format!(
        "{header}{}\n",
        waypoint::data::to_ron(&net).map_err(|d| vec![d])?
    );
    let scene_ron = format!(
        "{header}(\n  name: \"{name}\",\n  ground: (friction_scale: 1.0, gravity: 9.81),\n  roads: Some(\"{roads_rel}\"),\n  spawns: [\n    // Middle of lane {}.\n    (name: \"start\", position: ({:.2}, {:.2}), heading_deg: {:.1}),\n  ],\n)\n",
        lane.name, pos.x, pos.y, heading
    );
    for (rel, text) in [(&roads_rel, &roads_ron), (&scene_rel, &scene_ron)] {
        let path = ctx.project.join(rel);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&path, text));
        written.map_err(|e| {
            vec![Diagnostic::error(
                "WP-D001",
                format!("cannot write {}: {e}", path.display()),
            )]
        })?;
    }

    // Check the written files exactly as `wp check` would.
    let (_, diags) = Project::load(&ctx.project);
    if diags.has_errors() {
        return Err(diags.0);
    }

    if ctx.json {
        print_json(&json!({
            "ok": true,
            "roads_file": roads_rel,
            "scene_file": scene_rel,
            "osm_nodes": stats.osm_nodes,
            "osm_ways": stats.osm_ways,
            "drivable_ways": stats.drivable_ways,
            "roads": stats.roads,
            "junctions": stats.junctions,
            "lanes": built.lanes.len(),
            "connected_roads": connected_roads.len(),
            "clipped_to_bounds": stats.clipped,
            "extent_m": [stats.extent_m.0, stats.extent_m.1],
            "origin": { "lat": stats.origin.0, "lon": stats.origin.1 },
            "spawn": { "lane": lane.name, "position": [pos.x, pos.y], "heading_deg": heading },
        }));
    } else {
        println!(
            "imported {} of {} ways as {} roads ({} junctions, {} lanes), {:.0} × {:.0} m",
            stats.drivable_ways,
            stats.osm_ways,
            stats.roads,
            stats.junctions,
            built.lanes.len(),
            stats.extent_m.0,
            stats.extent_m.1
        );
        println!(
            "{} of {} roads are connected (routable from the spawn); the rest are dead ends or isolated",
            connected_roads.len(),
            stats.roads
        );
        if !stats.clipped {
            println!(
                "the file has no <bounds>, so ways were not clipped: export from openstreetmap.org or add [bbox:...] to an Overpass query"
            );
        }
        println!(
            "wrote {roads_rel} and {scene_rel} (spawn \"start\" on lane {})",
            lane.name
        );
        println!(
            "map data © OpenStreetMap contributors (ODbL); keep the attribution when you ship it"
        );
    }
    Ok(0)
}
