//! The `wp` subcommands, one module per group. Shared helpers live here;
//! each module brings them in with `use super::*`.

mod docs;
mod drive;
mod import;
mod info;
mod new;
mod project;
mod test;

pub use docs::docs;
pub(crate) use drive::make_session;
pub use drive::{DriveArgs, RunArgs, ServeArgs, ShotArgs, run, serve, shot};
pub use import::import_osm;
pub use info::{plugins, systems};
pub use new::new;
pub use project::{check, schema};
pub use test::{test, vehicle_test};

use crate::{CmdResult, Ctx, catalog};
use serde_json::json;
use std::path::Path;
use waypoint::app::App;
use waypoint::core::Diagnostic;
use waypoint::drive::Drive;
use waypoint::project::{GameConfig, Project};
use waypoint::test::{
    Assertion, Driver, InputKey, PlayerDef, ScenarioDef, ScenarioInput, ScenarioReport,
    run_scenario,
};
use waypoint::vehicle::{VehicleDef, VehicleInput};
use waypoint::world::{GroundDef, SceneDef, SpawnPoint};
use wp_render::CameraMode;

fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

/// Load the project; any error-level diagnostic aborts the command.
fn load_project(ctx: &Ctx) -> Result<Project, Vec<Diagnostic>> {
    let (project, diags) = Project::load(&ctx.project);
    if diags.has_errors() {
        let mut all = diags.0;
        all.push(
            Diagnostic::error("WP-G002", "the project has errors; fix them first")
                .help("run `wp check`"),
        );
        return Err(all);
    }
    Ok(project.expect("no errors means the project loaded"))
}

fn cli_error(message: impl Into<String>, help: impl Into<String>) -> Vec<Diagnostic> {
    vec![Diagnostic::error("WP-C001", message).help(help)]
}

fn known<'a>(items: impl Iterator<Item = &'a String>) -> String {
    let v: Vec<&str> = items.map(|s| s.as_str()).collect();
    format!(
        "known: {}",
        if v.is_empty() {
            "(none)".to_string()
        } else {
            v.join(", ")
        }
    )
}
