//! `wp new`: create a project from the sandbox template.

use super::*;

// ---- wp new ---------------------------------------------------------------

const TEMPLATE_SANDBOX: &[(&str, &[u8])] = &[
    (
        "assets/vehicles/hatchback.glb",
        include_bytes!("../../../../examples/sandbox/assets/vehicles/hatchback.glb"),
    ),
    (
        "assets/vehicles/hatchback.asset.ron",
        include_bytes!("../../../../examples/sandbox/assets/vehicles/hatchback.asset.ron"),
    ),
    (
        "assets/vehicles/city_bus.glb",
        include_bytes!("../../../../examples/sandbox/assets/vehicles/city_bus.glb"),
    ),
    (
        "assets/vehicles/city_bus.asset.ron",
        include_bytes!("../../../../examples/sandbox/assets/vehicles/city_bus.asset.ron"),
    ),
    (
        "game.toml",
        include_bytes!("../../../../examples/sandbox/game.toml"),
    ),
    (
        "vehicles/hatchback.vehicle.ron",
        include_bytes!("../../../../examples/sandbox/vehicles/hatchback.vehicle.ron"),
    ),
    (
        "vehicles/tractor.vehicle.ron",
        include_bytes!("../../../../examples/sandbox/vehicles/tractor.vehicle.ron"),
    ),
    (
        "trailers/semi_trailer.trailer.ron",
        include_bytes!("../../../../examples/sandbox/trailers/semi_trailer.trailer.ron"),
    ),
    (
        "roads/town.road.ron",
        include_bytes!("../../../../examples/sandbox/roads/town.road.ron"),
    ),
    (
        "scenes/town.scene.ron",
        include_bytes!("../../../../examples/sandbox/scenes/town.scene.ron"),
    ),
    (
        "scenery/town.scenery.ron",
        include_bytes!("../../../../examples/sandbox/scenery/town.scenery.ron"),
    ),
    (
        "tests/town_route.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/town_route.scenario.ron"),
    ),
    (
        "scenes/yard.scene.ron",
        include_bytes!("../../../../examples/sandbox/scenes/yard.scene.ron"),
    ),
    (
        "tests/truck_docking.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/truck_docking.scenario.ron"),
    ),
    (
        "tests/truck_brake_60.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/truck_brake_60.scenario.ron"),
    ),
    (
        "vehicles/city_bus.vehicle.ron",
        include_bytes!("../../../../examples/sandbox/vehicles/city_bus.vehicle.ron"),
    ),
    (
        "scenes/flat.scene.ron",
        include_bytes!("../../../../examples/sandbox/scenes/flat.scene.ron"),
    ),
    (
        "tests/accelerate_and_brake.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/accelerate_and_brake.scenario.ron"),
    ),
    (
        "tests/skidpad.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/skidpad.scenario.ron"),
    ),
    (
        "tests/car_brake_100.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/car_brake_100.scenario.ron"),
    ),
    (
        "tests/bus_brake_60.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/bus_brake_60.scenario.ron"),
    ),
    (
        "tests/bus_acceleration.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/bus_acceleration.scenario.ron"),
    ),
    (
        "roads/lane_drop.road.ron",
        include_bytes!("../../../../examples/sandbox/roads/lane_drop.road.ron"),
    ),
    (
        "scenes/lane_drop.scene.ron",
        include_bytes!("../../../../examples/sandbox/scenes/lane_drop.scene.ron"),
    ),
    (
        "tests/lane_drop.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/lane_drop.scenario.ron"),
    ),
    (
        "scenes/town_traffic.scene.ron",
        include_bytes!("../../../../examples/sandbox/scenes/town_traffic.scene.ron"),
    ),
    (
        "tests/town_traffic.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/town_traffic.scenario.ron"),
    ),
    (
        "scenes/crash.scene.ron",
        include_bytes!("../../../../examples/sandbox/scenes/crash.scene.ron"),
    ),
    (
        "tests/crash_into_bus.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/crash_into_bus.scenario.ron"),
    ),
    (
        "roads/village.road.ron",
        include_bytes!("../../../../examples/sandbox/roads/village.road.ron"),
    ),
    (
        "scenes/village.scene.ron",
        include_bytes!("../../../../examples/sandbox/scenes/village.scene.ron"),
    ),
    (
        "tests/village_traffic.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/village_traffic.scenario.ron"),
    ),
    (
        "missions/route_1.mission.ron",
        include_bytes!("../../../../examples/sandbox/missions/route_1.mission.ron"),
    ),
    (
        "missions/route_1.lua",
        include_bytes!("../../../../examples/sandbox/missions/route_1.lua"),
    ),
    (
        "tests/bus_route.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/bus_route.scenario.ron"),
    ),
    (
        "scenes/flat_wet.scene.ron",
        include_bytes!("../../../../examples/sandbox/scenes/flat_wet.scene.ron"),
    ),
    (
        "tests/car_brake_100_wet.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/car_brake_100_wet.scenario.ron"),
    ),
    (
        "scenes/town_rain.scene.ron",
        include_bytes!("../../../../examples/sandbox/scenes/town_rain.scene.ron"),
    ),
    (
        "tests/keyboard_slalom.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/keyboard_slalom.scenario.ron"),
    ),
    (
        "tests/keyboard_brake_tap.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/keyboard_brake_tap.scenario.ron"),
    ),
    (
        "tests/bus_turning_circle.scenario.ron",
        include_bytes!("../../../../examples/sandbox/tests/bus_turning_circle.scenario.ron"),
    ),
    (
        "AGENTS.md",
        include_bytes!("../../../../examples/sandbox/AGENTS.md"),
    ),
];

/// The `minimal` template: one car on a flat test ground with two tests,
/// for starting a game from scratch.
const TEMPLATE_MINIMAL: &[&str] = &[
    "game.toml",
    "AGENTS.md",
    "vehicles/hatchback.vehicle.ron",
    "assets/vehicles/hatchback.glb",
    "assets/vehicles/hatchback.asset.ron",
    "scenes/flat.scene.ron",
    "tests/accelerate_and_brake.scenario.ron",
    "tests/skidpad.scenario.ron",
];

pub fn new(ctx: &Ctx, dest: &Path, template: &str) -> CmdResult {
    let files: Vec<&(&str, &[u8])> = match template {
        "sandbox" => TEMPLATE_SANDBOX.iter().collect(),
        "minimal" => TEMPLATE_SANDBOX
            .iter()
            .filter(|(rel, _)| TEMPLATE_MINIMAL.contains(rel))
            .collect(),
        _ => {
            return Err(cli_error(
                format!("unknown template `{template}`"),
                "available templates: sandbox (everything: town, traffic, bus route, truck), minimal (a car on a flat ground)",
            ));
        }
    };
    if dest.exists() {
        return Err(vec![
            Diagnostic::error("WP-C004", format!("{} already exists", dest.display()))
                .help("choose a new folder name"),
        ]);
    }
    let game_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "game".into());
    let mut written = Vec::new();
    for (rel, content) in files {
        let path = dest.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| io_error(&path, e))?;
        let content: Vec<u8> = if *rel == "game.toml" {
            String::from_utf8_lossy(content)
                .replacen("name = \"sandbox\"", &format!("name = \"{game_name}\""), 1)
                .into_bytes()
        } else {
            content.to_vec()
        };
        std::fs::write(&path, content).map_err(|e| io_error(&path, e))?;
        written.push(rel.to_string());
    }
    if ctx.json {
        print_json(&json!({ "ok": true, "path": dest, "files": written }));
    } else {
        println!("created {} ({} files)", dest.display(), written.len());
        println!("next: cd {} && wp check && wp test", dest.display());
    }
    Ok(0)
}

fn io_error(path: &Path, e: std::io::Error) -> Vec<Diagnostic> {
    vec![Diagnostic::error(
        "WP-D001",
        format!("cannot write {}: {e}", path.display()),
    )]
}
