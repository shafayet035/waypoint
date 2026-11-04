//! `wp check` and `wp schema`: validating project files and printing their schemas.

use super::*;

// ---- wp check -------------------------------------------------------------

pub fn check(ctx: &Ctx) -> CmdResult {
    let start = std::time::Instant::now();
    let (project, diags) = Project::load(&ctx.project);
    let files = project
        .as_ref()
        .map(|p| 1 + p.vehicles.len() + p.scenes.len() + p.scenarios.len())
        .unwrap_or(0);
    let ms = start.elapsed().as_millis();
    if ctx.json {
        print_json(&json!({
            "ok": !diags.has_errors(),
            "files": files,
            "errors": diags.error_count(),
            "warnings": diags.warning_count(),
            "elapsed_ms": ms,
            "diagnostics": diags,
        }));
    } else {
        for d in diags.iter() {
            print!("{}", d.render());
        }
        println!(
            "checked {files} files in {ms} ms: {} error(s), {} warning(s)",
            diags.error_count(),
            diags.warning_count()
        );
    }
    Ok(if diags.has_errors() { 1 } else { 0 })
}

// ---- wp schema ------------------------------------------------------------

pub fn schema(_ctx: &Ctx, kind: &str) -> CmdResult {
    use waypoint::data::schema_json;
    let text = match kind {
        "game" => schema_json::<GameConfig>(),
        "vehicle" => schema_json::<VehicleDef>(),
        "scene" => schema_json::<SceneDef>(),
        "scenario" => schema_json::<ScenarioDef>(),
        "trailer" => schema_json::<waypoint::vehicle::TrailerDef>(),
        "road" => schema_json::<waypoint::road::RoadNetworkDef>(),
        "mission" => schema_json::<waypoint::mission::MissionDef>(),
        other => {
            return Err(cli_error(
                format!("unknown schema type `{other}`"),
                "types: game, vehicle, trailer, road, scene, scenario, mission",
            ));
        }
    };
    println!("{text}");
    Ok(0)
}
