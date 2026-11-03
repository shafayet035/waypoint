//! `waypoint-player`: plays a project in a window, with keyboard or touch.
//!
//! Finds the project in this order: the first argument that is a folder, the
//! `WP_PROJECT` environment variable, a `game` folder next to the executable
//! (how iOS and Android bundles ship it), then the current folder.
//! `--scenario <name>` plays a scenario with its scripted driver (a demo mode).

use std::path::{Path, PathBuf};
use waypoint::core::Diagnostic;
use waypoint::project::Project;

fn find_project(args: &[String]) -> PathBuf {
    let candidates = args
        .iter()
        .map(PathBuf::from)
        .chain(std::env::var_os("WP_PROJECT").map(PathBuf::from))
        .chain(
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.join("game"))),
        )
        .chain(std::iter::once(PathBuf::from(".")));
    for c in candidates {
        if c.join("game.toml").is_file() {
            return c;
        }
    }
    PathBuf::from(".")
}

fn run(args: &[String]) -> Result<(), Vec<Diagnostic>> {
    let scenario = args
        .iter()
        .position(|a| a == "--scenario")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let dirs: Vec<String> = args
        .iter()
        .filter(|a| Path::new(a).is_dir())
        .cloned()
        .collect();
    let root = find_project(&dirs);
    let (project, diags) = Project::load(&root);
    for d in diags.iter() {
        eprint!("{}", d.render());
    }
    let project = match project {
        Some(p) if !diags.has_errors() => p,
        _ => return Err(diags.0),
    };
    let (app, player) = waypoint::start(&project, scenario.as_deref(), 0).map_err(|d| vec![d])?;
    let vehicles = project
        .vehicles
        .iter()
        .map(|(k, v)| (k.clone(), v.value.clone()))
        .collect();
    let session = wp_control::Session::new(app, Some(player), vehicles);
    wp_platform::run_windowed(
        session,
        wp_platform::WindowOptions {
            title: project.game.value.name.clone(),
            width: 1280,
            height: 720,
            camera: wp_render::CameraMode::Chase,
            exit_after_s: std::env::var("WP_EXIT_AFTER")
                .ok()
                .and_then(|s| s.parse().ok()),
            control: None,
        },
    )
    .map_err(|d| vec![d])?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(diags) = run(&args) {
        for d in diags {
            eprint!("{}", d.render());
        }
        std::process::exit(1);
    }
}
