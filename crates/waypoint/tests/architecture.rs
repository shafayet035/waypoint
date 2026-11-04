//! Enforces ADR 0001: each crate may depend only on the crates its layer allows,
//! and simulation crates may not pull in graphics or windowing libraries.

use std::collections::BTreeMap;
use std::path::Path;

/// Internal crates each crate may depend on (directly).
fn allowed() -> BTreeMap<&'static str, Vec<&'static str>> {
    let foundation = vec!["wp_core", "wp_ecs", "wp_app", "wp_data"];
    let simulation = [
        foundation.clone(),
        vec![
            "wp_physics",
            "wp_road",
            "wp_world",
            "wp_vehicle",
            "wp_traffic",
        ],
    ]
    .concat();
    let gameplay = [simulation.clone(), vec!["wp_script", "wp_mission"]].concat();
    let verification = [gameplay.clone(), vec!["wp_test"]].concat();
    let presentation = [gameplay.clone(), vec!["wp_asset", "wp_rhi", "wp_render"]].concat();
    let agent = [presentation.clone(), vec!["wp_control"]].concat();
    let platform = [agent.clone(), vec!["wp_platform"]].concat();
    let everything = [
        platform.clone(),
        verification.clone(),
        vec!["waypoint", "wp_player"],
    ]
    .concat();
    BTreeMap::from([
        ("wp_core", vec![]),
        ("wp_ecs", vec!["wp_core"]),
        ("wp_app", vec!["wp_core", "wp_ecs"]),
        ("wp_data", vec!["wp_core"]),
        ("wp_physics", foundation.clone()),
        ("wp_road", foundation.clone()),
        (
            "wp_world",
            [foundation.clone(), vec!["wp_physics", "wp_road"]].concat(),
        ),
        (
            "wp_vehicle",
            [foundation.clone(), vec!["wp_physics"]].concat(),
        ),
        (
            "wp_traffic",
            [
                foundation.clone(),
                vec!["wp_physics", "wp_road", "wp_world", "wp_vehicle"],
            ]
            .concat(),
        ),
        ("wp_script", foundation.clone()),
        (
            "wp_mission",
            [simulation.clone(), vec!["wp_script"]].concat(),
        ),
        ("wp_test", verification),
        ("wp_asset", vec!["wp_core", "wp_data"]),
        ("wp_rhi", vec!["wp_core"]),
        ("wp_render", presentation),
        ("wp_control", agent),
        ("wp_platform", platform),
        ("waypoint", everything.clone()),
        ("wp_cli", everything.clone()),
        ("wp_player", everything),
    ])
}

/// External libraries the simulation must never depend on.
const NOT_IN_SIMULATION: &[&str] = &["wgpu", "winit", "png", "pollster"];
const SIMULATION_LAYERS: &[&str] = &[
    "wp_core",
    "wp_ecs",
    "wp_app",
    "wp_data",
    "wp_physics",
    "wp_road",
    "wp_world",
    "wp_vehicle",
    "wp_traffic",
    "wp_script",
    "wp_mission",
    "wp_test",
];

/// `[dependencies]` names of a Cargo.toml (not dev-dependencies).
fn dependencies(manifest: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]"
                || (t.starts_with("[target.") && t.ends_with(".dependencies]"));
            continue;
        }
        if in_deps
            && let Some((name, _)) = t.split_once('=')
            && !t.starts_with('#')
        {
            // `wp_core.workspace = true` and `wp_core = { ... }` both name `wp_core`.
            let name = name.trim().trim_matches('"');
            out.push(name.split('.').next().unwrap_or(name).to_string());
        }
    }
    out
}

#[test]
fn crates_respect_their_layers() {
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let allowed = allowed();
    let mut violations = Vec::new();
    let mut seen = Vec::new();
    for entry in std::fs::read_dir(&crates_dir).unwrap().flatten() {
        let manifest = entry.path().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().to_string();
        seen.push(name.clone());
        let Some(ok) = allowed.get(name.as_str()) else {
            violations.push(format!(
                "{name}: not assigned to a layer in tests/architecture.rs (and ADR 0001)"
            ));
            continue;
        };
        for dep in dependencies(&text) {
            let internal = dep.starts_with("wp_") || dep == "waypoint";
            if internal && !ok.contains(&dep.as_str()) {
                violations.push(format!("{name} → {dep}: not allowed by its layer"));
            }
            if SIMULATION_LAYERS.contains(&name.as_str())
                && NOT_IN_SIMULATION.contains(&dep.as_str())
            {
                violations.push(format!(
                    "{name} → {dep}: simulation must not depend on graphics or windows"
                ));
            }
        }
    }
    assert!(
        seen.len() >= 15,
        "expected to find all crates, found {seen:?}"
    );
    assert!(
        violations.is_empty(),
        "architecture violations:\n{}",
        violations.join("\n")
    );
}
