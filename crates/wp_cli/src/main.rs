//! `wp`: the Waypoint command line. Every command accepts `--json` and returns
//! a stable structure. Exit code 0 = success, 1 = errors or failed tests,
//! 2 = bad command-line usage.

mod catalog;
mod commands;
mod mcp;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use waypoint::core::Diagnostic;

#[derive(Parser)]
#[command(
    name = "wp",
    version,
    about = "Waypoint: an agent-first driving engine",
    after_help = "Start with `wp docs` for topics, or `wp check` inside a project."
)]
struct Cli {
    /// Project folder (contains game.toml).
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,
    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new project from a template.
    New {
        /// Folder to create; its last part becomes the game name.
        name: PathBuf,
        #[arg(long, default_value = "sandbox")]
        template: String,
    },
    /// Validate every project file against its schema and cross-references.
    Check,
    /// Print the JSON Schema of a data type: game, vehicle, trailer, road, scene, scenario.
    Schema { kind: String },
    /// Offline docs. Topics: cli, errors, errors/<CODE>, scenario, vehicle.
    Docs { topic: Option<String> },
    /// Drive in a window (keyboard), or --headless to simulate and print the final state.
    Run(commands::RunArgs),
    /// Render one frame to a PNG, headless: any scenario or drive at any time.
    Shot(commands::ShotArgs),
    /// Control protocol on stdin/stdout (JSON-RPC 2.0, one request per line).
    Serve(commands::ServeArgs),
    /// Model Context Protocol server on stdin/stdout, for Claude and other agents.
    Mcp,
    /// Run scenario tests (all, or those whose name contains FILTER).
    Test { filter: Option<String> },
    /// Print the resolved system schedule.
    Systems,
    /// Print loaded plugins in load order.
    Plugins,
    /// Vehicle tools.
    #[command(subcommand)]
    Vehicle(VehicleCmd),
    /// Import map data into the project.
    #[command(subcommand)]
    Import(ImportCmd),
}

#[derive(Subcommand)]
enum ImportCmd {
    /// OpenStreetMap XML (.osm) → roads/<name>.road.ron + scenes/<name>.scene.ron.
    Osm {
        /// The .osm file (openstreetmap.org Export, or an Overpass API query).
        file: PathBuf,
        /// Name of the road network and scene.
        #[arg(long)]
        name: String,
        /// Which side traffic drives on: right or left.
        #[arg(long, default_value = "right")]
        drive_side: String,
        /// Prefer road names in this language (`name:<lang>` tags), e.g. en.
        #[arg(long)]
        lang: Option<String>,
        /// Overwrite existing files.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum VehicleCmd {
    /// Run standard manoeuvres headless: accel_0_100, brake_100_0, skidpad, or all.
    Test {
        name: String,
        #[arg(long, default_value = "all")]
        manoeuvre: String,
    },
}

pub struct Ctx {
    pub project: PathBuf,
    pub json: bool,
}

/// What a command returns: an exit code, or diagnostics to print (exit 1).
pub type CmdResult = Result<i32, Vec<Diagnostic>>;

fn main() {
    let cli = Cli::parse();
    let ctx = Ctx {
        project: cli.project,
        json: cli.json,
    };
    let result = match cli.cmd {
        Cmd::New { name, template } => commands::new(&ctx, &name, &template),
        Cmd::Check => commands::check(&ctx),
        Cmd::Schema { kind } => commands::schema(&ctx, &kind),
        Cmd::Docs { topic } => commands::docs(&ctx, topic.as_deref()),
        Cmd::Run(args) => commands::run(&ctx, &args),
        Cmd::Shot(args) => commands::shot(&ctx, &args),
        Cmd::Serve(args) => commands::serve(&ctx, &args),
        Cmd::Mcp => mcp::serve(&ctx),
        Cmd::Test { filter } => commands::test(&ctx, filter.as_deref()),
        Cmd::Systems => commands::systems(&ctx),
        Cmd::Plugins => commands::plugins(&ctx),
        Cmd::Import(ImportCmd::Osm {
            file,
            name,
            drive_side,
            lang,
            force,
        }) => commands::import_osm(&ctx, &file, &name, &drive_side, lang.as_deref(), force),
        Cmd::Vehicle(VehicleCmd::Test { name, manoeuvre }) => {
            commands::vehicle_test(&ctx, &name, &manoeuvre)
        }
    };
    let code = match result {
        Ok(code) => code,
        Err(diags) => {
            if ctx.json {
                let out = serde_json::json!({ "ok": false, "diagnostics": diags });
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
            } else {
                for d in &diags {
                    eprint!("{}", d.render());
                }
            }
            1
        }
    };
    std::process::exit(code);
}
