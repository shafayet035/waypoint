//! `wp import osm` and `wp import opendrive`: turn map data into a road
//! network and a scene. The importers are pure (`waypoint::road`); this
//! module checks the project, places a spawn, writes the files and reports.

use super::*;
use waypoint::road::{DriveSide, RoadNetwork, RoadNetworkDef, opendrive, osm};

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
    let xml = read_source(
        ctx,
        file,
        "export an area from openstreetmap.org (Export → .osm) or the Overpass API",
    )?;
    let (net, scenery, stats) =
        osm::import_with_buildings(&xml, name, side, lang).map_err(|e| {
            import_error(
                file,
                e,
                "the file must be OSM XML (.osm) with highway=* ways and their nodes",
            )
        })?;
    let header = format!(
        "// Imported from OpenStreetMap ({}) by `wp import osm`.\n\
         // Map data © OpenStreetMap contributors, available under the Open Database\n\
         // License (ODbL): https://www.openstreetmap.org/copyright\n\
         // Origin (0, 0) = latitude {:.6}, longitude {:.6}; x = east, y = north, metres.\n",
        file_name(file),
        stats.origin.0,
        stats.origin.1
    );
    let mut lines = vec![format!(
        "imported {} of {} ways as {} roads ({} junctions), {:.0} × {:.0} m",
        stats.drivable_ways,
        stats.osm_ways,
        stats.roads,
        stats.junctions,
        stats.extent_m.0,
        stats.extent_m.1
    )];
    if !stats.clipped {
        lines.push(
            "the file has no <bounds>, so ways were not clipped: export from openstreetmap.org or add [bbox:...] to an Overpass query".into(),
        );
    }
    let report = json!({
        "osm_nodes": stats.osm_nodes,
        "osm_ways": stats.osm_ways,
        "drivable_ways": stats.drivable_ways,
        "junctions": stats.junctions,
        "clipped_to_bounds": stats.clipped,
        "extent_m": [stats.extent_m.0, stats.extent_m.1],
        "origin": { "lat": stats.origin.0, "lon": stats.origin.1 },
    });
    write_import(
        ctx,
        Imported {
            scenery,
            net,
            name,
            force,
            header,
            report,
            lines,
            credit: Some(
                "map data © OpenStreetMap contributors (ODbL); keep the attribution when you ship it",
            ),
        },
    )
}

pub fn import_opendrive(ctx: &Ctx, file: &Path, name: &str, force: bool) -> CmdResult {
    let xml = read_source(
        ctx,
        file,
        "give an ASAM OpenDRIVE file (.xodr), as exported by RoadRunner, CARLA, esmini or similar tools",
    )?;
    let (net, stats) = opendrive::import(&xml, name).map_err(|e| {
        import_error(
            file,
            e,
            "the file must be ASAM OpenDRIVE (.xodr) with roads that have driving lanes",
        )
    })?;
    let shifted = stats.shift != (0.0, 0.0);
    let header = format!(
        "// Imported from OpenDRIVE ({}) by `wp import opendrive`.\n\
         // Keep the source file's licence and credit with the game.\n{}",
        file_name(file),
        if shifted {
            format!(
                "// Coordinates moved by ({:.2}, {:.2}) m to centre them on (0, 0).\n",
                -stats.shift.0, -stats.shift.1
            )
        } else {
            String::new()
        }
    );
    let mut lines = vec![format!(
        "imported {} of {} roads as {} ({} junctions; {} junction roads rebuilt by the engine, {} without driving lanes skipped), {:.0} × {:.0} m",
        stats.roads_in_file - stats.connecting_roads - stats.skipped_roads,
        stats.roads_in_file,
        stats.roads,
        stats.junctions_in_file,
        stats.connecting_roads,
        stats.skipped_roads,
        stats.extent_m.0,
        stats.extent_m.1
    )];
    lines.extend(stats.notes.iter().map(|n| format!("note: {n}")));
    let report = json!({
        "roads_in_file": stats.roads_in_file,
        "junctions_in_file": stats.junctions_in_file,
        "connecting_roads": stats.connecting_roads,
        "skipped_roads": stats.skipped_roads,
        "shift_m": [stats.shift.0, stats.shift.1],
        "extent_m": [stats.extent_m.0, stats.extent_m.1],
        "notes": stats.notes,
    });
    write_import(
        ctx,
        Imported {
            net,
            name,
            force,
            header,
            report,
            lines,
            credit: None,
            scenery: Default::default(),
        },
    )
}

/// An importer's result, ready to write.
struct Imported<'a> {
    net: RoadNetworkDef,
    name: &'a str,
    force: bool,
    /// Comment lines at the top of both files (source, licence, origin).
    header: String,
    /// Importer-specific fields for `--json`.
    report: serde_json::Value,
    /// Importer-specific lines for the text report.
    lines: Vec<String>,
    /// A credit line to print last, if the data needs one.
    credit: Option<&'static str>,
    /// Buildings and trees from the map, if it has any.
    scenery: osm::OsmScenery,
}

/// A building style for an OSM `building=*` kind; plain "yes" buildings get
/// one of the common styles, picked by id so neighbours differ.
fn osm_style(kind: &str, id: &str) -> &'static str {
    match kind {
        "house" | "detached" | "semidetached_house" | "terrace" | "bungalow" | "cabin" | "farm"
        | "hut" | "mosque" | "church" | "temple" | "cathedral" | "chapel" => "plaster",
        "apartments" | "residential" | "dormitory" | "hotel" | "school" | "university"
        | "college" | "hospital" | "kindergarten" | "public" | "civic" | "construction" => {
            "concrete"
        }
        "commercial" | "retail" | "supermarket" | "kiosk" | "shop" => "shophouse",
        "office" | "government" | "bank" => "office",
        "industrial" | "warehouse" | "factory" | "manufacture" | "hangar" | "garage"
        | "garages" | "shed" | "service" | "storage_tank" => "industrial",
        _ => {
            let h = id
                .bytes()
                .fold(0u32, |h, b| h.wrapping_mul(131).wrapping_add(b as u32));
            ["plaster", "concrete", "plaster", "brick"][(h % 4) as usize]
        }
    }
}

/// The imported buildings as a scenery file, without those standing on the
/// imported roads (OSM roads have no width, so ours can reach into the
/// buildings beside narrow streets). Returns (file text, buildings kept,
/// buildings dropped, trees).
fn scenery_file(
    header: &str,
    name: &str,
    map: &osm::OsmScenery,
    net: &RoadNetwork,
) -> (String, usize, usize, usize) {
    let buildings = &map.buildings;
    use waypoint::world::scenery::{BuildingDef, SceneryDef, buildings_on_roads};
    let def = SceneryDef {
        name: name.to_string(),
        buildings: buildings
            .iter()
            .map(|b| BuildingDef {
                footprint: b.footprint.clone(),
                height_m: b.height_m,
                style: Some(osm_style(&b.kind, &b.id).to_string()),
            })
            .collect(),
        ..Default::default()
    };
    let on_road: std::collections::BTreeSet<usize> = buildings_on_roads(&def, net)
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    let mut text = format!(
        "{header}// Buildings from the map's building=* outlines; heights from height or\n\
         // building:levels tags, else typical for the kind. Buildings standing on a road are left out.\n(\n  name: \"{name}\",\n  buildings: [\n"
    );
    let mut kept = 0;
    for (i, (b, osm)) in def.buildings.iter().zip(buildings).enumerate() {
        if on_road.contains(&i) {
            continue;
        }
        kept += 1;
        let pts: Vec<String> = b
            .footprint
            .iter()
            .map(|(x, y)| format!("({x:.2}, {y:.2})"))
            .collect();
        text.push_str(&format!(
            "    (footprint: [{}], height_m: {:.1}, style: Some(\"{}\")), // way {} ({})\n",
            pts.join(", "),
            b.height_m,
            b.style.as_deref().unwrap_or("plaster"),
            osm.id,
            osm.kind
        ));
    }
    text.push_str("  ],\n");
    // Trees from natural=tree points; any on a carriageway are left out.
    let trees: Vec<String> = map
        .trees
        .iter()
        .filter(|t| {
            let p = waypoint::core::glam::Vec2::new(t.position.0, t.position.1);
            net.roads
                .iter()
                .all(|r| r.centre.project(p).2 > r.left_m.max(r.right_m) + 0.3)
        })
        .map(|t| {
            let kind = if t.palm {
                ", kind: Palm"
            } else if t.needles {
                ", kind: Cone"
            } else {
                ""
            };
            format!(
                "    (position: ({:.2}, {:.2}), height_m: {:.1}{kind}),\n",
                t.position.0, t.position.1, t.height_m
            )
        })
        .collect();
    if !trees.is_empty() {
        text.push_str("  trees: [\n");
        text.push_str(&trees.concat());
        text.push_str("  ],\n");
    }
    text.push_str(")\n");
    (text, kept, on_road.len(), trees.len())
}

fn read_source(ctx: &Ctx, file: &Path, help: &str) -> Result<String, Vec<Diagnostic>> {
    if !ctx.project.join("game.toml").exists() {
        return Err(vec![
            Diagnostic::error(
                "WP-G001",
                format!("no game.toml in {}", ctx.project.display()),
            )
            .help("pass --project <dir>, or create one with `wp new <name>`"),
        ]);
    }
    std::fs::read_to_string(file).map_err(|e| {
        vec![
            Diagnostic::error("WP-D001", format!("cannot read {}: {e}", file.display())).help(help),
        ]
    })
}

fn import_error(file: &Path, e: String, help: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::error("WP-I001", format!("cannot import {}: {e}", file.display())).help(help)]
}

fn file_name(file: &Path) -> String {
    file.file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Place a spawn, write `roads/<name>.road.ron` and `scenes/<name>.scene.ron`,
/// check the project and report.
fn write_import(ctx: &Ctx, imp: Imported) -> CmdResult {
    let Imported {
        net,
        name,
        force,
        header,
        mut report,
        lines,
        credit,
        scenery: map_scenery,
    } = imp;
    // A spawn point halfway along a long lane from which the most of the
    // map can be reached (edges of the data are often dead ends).
    let built = RoadNetwork::build(&net);
    let (lane, reachable_roads) = built.best_start(64).ok_or_else(|| {
        vec![
            Diagnostic::error("WP-I001", "the imported roads have no lanes")
                .help("pick a larger area with at least one drivable road"),
        ]
    })?;
    let lane = &built.lanes[lane];
    let (pos, dir) = lane.path.sample(lane.path.length() / 2.0);
    let heading = dir.y.atan2(dir.x).to_degrees();

    let roads_rel = format!("roads/{name}.road.ron");
    let scene_rel = format!("scenes/{name}.scene.ron");
    let scenery_rel = format!("scenery/{name}.scenery.ron");
    let scenery = (!map_scenery.buildings.is_empty() || !map_scenery.trees.is_empty())
        .then(|| scenery_file(&header, name, &map_scenery, &built));
    for rel in [&roads_rel, &scene_rel]
        .into_iter()
        .chain(scenery.as_ref().map(|_| &scenery_rel))
    {
        if ctx.project.join(rel).exists() && !force {
            return Err(cli_error(
                format!("{rel} already exists"),
                "pick another --name, or pass --force to overwrite",
            ));
        }
    }
    let roads_ron = format!(
        "{header}{}\n",
        waypoint::data::to_ron(&net).map_err(|d| vec![d])?
    );
    let scenery_line = if scenery.is_some() {
        format!("  scenery: Some(\"{scenery_rel}\"),\n")
    } else {
        String::new()
    };
    let scene_ron = format!(
        "{header}(\n  name: \"{name}\",\n  ground: (friction_scale: 1.0, gravity: 9.81),\n  roads: Some(\"{roads_rel}\"),\n{scenery_line}  spawns: [\n    // Middle of lane {}.\n    (name: \"start\", position: ({:.2}, {:.2}), heading_deg: {:.1}),\n  ],\n)\n",
        lane.name, pos.x, pos.y, heading
    );
    let mut files = vec![(&roads_rel, roads_ron), (&scene_rel, scene_ron)];
    if let Some((text, ..)) = &scenery {
        files.push((&scenery_rel, text.clone()));
    }
    for (rel, text) in files {
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
        let common = json!({
            "ok": true,
            "roads_file": roads_rel,
            "scene_file": scene_rel,
            "drive_side": format!("{:?}", net.drive_side),
            "roads": net.roads.len(),
            "lanes": built.lanes.len(),
            "reachable_roads": reachable_roads.len(),
            "spawn": { "lane": lane.name, "position": [pos.x, pos.y], "heading_deg": heading },
            "scenery_file": scenery.as_ref().map(|_| &scenery_rel),
            "buildings": scenery.as_ref().map_or(0, |s| s.1),
            "buildings_on_roads_dropped": scenery.as_ref().map_or(0, |s| s.2),
            "trees": scenery.as_ref().map_or(0, |s| s.3),
        });
        if let (Some(r), Some(c)) = (report.as_object_mut(), common.as_object()) {
            r.extend(c.clone());
        }
        print_json(&report);
    } else {
        for l in &lines {
            println!("{l}");
        }
        println!(
            "{} of {} roads can be reached from the spawn (one-way streets and dead ends limit the rest)",
            reachable_roads.len(),
            net.roads.len()
        );
        println!(
            "wrote {roads_rel} and {scene_rel} (spawn \"start\" on lane {})",
            lane.name
        );
        if let Some((_, kept, dropped, trees)) = &scenery {
            println!(
                "wrote {scenery_rel}: {kept} buildings ({dropped} standing on the imported roads left out), {trees} trees"
            );
        }
        if let Some(c) = credit {
            println!("{c}");
        }
    }
    Ok(0)
}
