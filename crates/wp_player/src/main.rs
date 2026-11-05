//! `waypoint-player` for desktop and iOS. Finds the project in this order: a
//! folder argument, `WP_PROJECT`, a `game` folder next to the executable (how
//! mobile bundles ship it), then the current folder. `--scenario <name>` plays a
//! scenario with its scripted driver; `WP_EXIT_AFTER=<s>` closes it (smoke tests).

use std::path::{Path, PathBuf};

fn find_project(args: &[String]) -> PathBuf {
    let candidates = args
        .iter()
        .filter(|a| Path::new(a).is_dir())
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenario = args
        .iter()
        .position(|a| a == "--scenario")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let exit_after = std::env::var("WP_EXIT_AFTER")
        .ok()
        .and_then(|s| s.parse().ok());
    let root = find_project(&args);
    if let Err(diags) = wp_player::play(
        &root,
        None,
        scenario.as_deref(),
        exit_after,
        wp_platform::run_windowed,
    ) {
        for d in diags {
            eprint!("{}", d.render());
        }
        std::process::exit(1);
    }
}
