//! `wp test` (scenario files) and `wp vehicle test` (standard manoeuvres).

use super::*;

// ---- wp test --------------------------------------------------------------

pub fn test(ctx: &Ctx, filter: Option<&str>, approve: bool) -> CmdResult {
    let project = load_project(ctx)?;
    let matches = |name: &String| filter.is_none_or(|f| name.contains(f));
    let shots: Vec<&String> = project.shots.keys().filter(|n| matches(n)).collect();
    if approve {
        if shots.is_empty() {
            return Err(vec![
                Diagnostic::error(
                    "WP-C003",
                    format!("no screenshot test matches `{}`", filter.unwrap_or("")),
                )
                .help(known(project.shots.keys())),
            ]);
        }
        return super::shots::approve(ctx, &project, &shots);
    }
    let selected: Vec<_> = project
        .scenarios
        .iter()
        .filter(|(n, _)| matches(n))
        .collect();
    if selected.is_empty() && shots.is_empty() {
        return Err(vec![
            Diagnostic::error(
                "WP-C003",
                format!(
                    "no scenario or screenshot test matches `{}`",
                    filter.unwrap_or("")
                ),
            )
            .help(known(project.scenarios.keys().chain(project.shots.keys()))),
        ]);
    }
    let make_app = || waypoint::make_app(project.disabled_plugins());
    let mut reports: Vec<(String, ScenarioReport)> = Vec::new();
    for (name, sc) in selected {
        let input = project.scenario_input(name).ok_or_else(|| {
            cli_error(format!("scenario `{name}` is incomplete"), "run `wp check`")
        })?;
        let mut report = run_scenario(&input, &make_app).map_err(|d| vec![d])?;
        if let Some(golden) = project.golden_replay(name) {
            report.check_golden(&golden.value, &golden.path, 60);
        }
        if !ctx.json {
            print_report(&report, &sc.path);
        }
        reports.push((sc.path.clone(), report));
    }
    let shot_results = if shots.is_empty() {
        Vec::new()
    } else {
        let gpu = wp_rhi::Gpu::headless();
        super::shots::run_shots(&project, &shots, &gpu)?
    };
    if !ctx.json {
        for r in &shot_results {
            super::shots::print_shot(r);
        }
    }
    let failed = reports.iter().filter(|(_, r)| !r.passed).count()
        + shot_results.iter().filter(|r| !r.passed).count();
    let total = reports.len() + shot_results.len();
    let skipped = shot_results.iter().filter(|r| r.skipped.is_some()).count();
    if ctx.json {
        let list: Vec<_> = reports
            .iter()
            .map(|(path, r)| json!({ "path": path, "report": r }))
            .collect();
        print_json(&json!({
            "passed": failed == 0,
            "total": total,
            "failed": failed,
            "skipped": skipped,
            "reports": list,
            "shots": shot_results,
        }));
    } else {
        println!(
            "\ntest result: {}. {} passed; {} failed{}",
            if failed == 0 { "ok" } else { "FAILED" },
            total - failed - skipped,
            failed,
            if skipped > 0 {
                format!("; {skipped} skipped (no GPU)")
            } else {
                String::new()
            }
        );
    }
    Ok(if failed == 0 { 0 } else { 1 })
}

fn print_report(r: &ScenarioReport, path: &str) {
    println!(
        "test {} ... {} ({} ms)",
        r.name,
        if r.passed { "ok" } else { "FAILED" },
        r.wall_ms
    );
    for a in &r.assertions {
        println!(
            "  {} {:<36} {}",
            if a.passed { "✓" } else { "✗" },
            a.assertion,
            a.detail
        );
    }
    if !r.passed {
        println!("  --> {path}");
        println!(
            "  final state {}; rerun one test with `wp test {}`",
            r.final_hash, r.name
        );
    }
}

// ---- wp vehicle test --------------------------------------------------------

pub const MANOEUVRES: [&str; 6] = [
    "accel_0_50",
    "accel_0_100",
    "brake_60_0",
    "brake_100_0",
    "skidpad",
    "turning_circle",
];

fn manoeuvre(name: &str, vehicle: &str) -> Option<ScenarioDef> {
    let player = |initial_kmh: f32, driver: Driver| PlayerDef {
        vehicle: vehicle.to_string(),
        trailer: None,
        spawn: "origin".into(),
        initial_kmh,
        driver,
    };
    let key = |at_s: f32, throttle: f32, brake: f32| InputKey {
        reverse: false,
        at_s,
        throttle,
        brake,
        steer: 0.0,
    };
    let accel = |kmh: f32| {
        (
            player(0.0, Driver::Script(vec![key(0.0, 1.0, 0.0)])),
            60.0,
            vec![
                Assertion::ReachSpeed {
                    kmh,
                    within_s: 60.0,
                    not_before_s: 0.0,
                },
                Assertion::NoNaN,
            ],
        )
    };
    let brake = |kmh: f32| {
        (
            player(kmh, Driver::Script(vec![key(0.0, 0.0, 1.0)])),
            20.0,
            vec![
                Assertion::BrakingDistance { max_m: 1000.0 },
                Assertion::StoppedBy { s: 20.0 },
                Assertion::NoNaN,
            ],
        )
    };
    let (player, duration_s, assert) = match name {
        "accel_0_50" => accel(50.0),
        "accel_0_100" => accel(100.0),
        "brake_60_0" => brake(60.0),
        "brake_100_0" => brake(100.0),
        "skidpad" => (
            player(
                0.0,
                Driver::Cruise {
                    kmh: 40.0,
                    steer: 0.25,
                },
            ),
            30.0,
            vec![Assertion::NoNaN],
        ),
        // Full lock at walking pace: the path of the outer front wheel.
        "turning_circle" => (
            player(
                0.0,
                Driver::Cruise {
                    kmh: 6.0,
                    steer: 1.0,
                },
            ),
            75.0,
            vec![Assertion::NoNaN],
        ),
        _ => return None,
    };
    Some(ScenarioDef {
        scene: "<flat>".into(),
        seed: 0,
        player,
        duration_s,
        mission: None,
        assert,
    })
}

/// Manoeuvres that make sense for this vehicle (a bus governed to 80 km/h
/// cannot do the 100 km/h tests).
fn default_manoeuvres(def: &VehicleDef) -> Vec<&'static str> {
    let fast = def.engine.governed_speed_kmh.is_none_or(|g| g > 105.0);
    MANOEUVRES
        .iter()
        .copied()
        .filter(|m| fast || !m.contains("100"))
        .collect()
}

pub fn vehicle_test(ctx: &Ctx, name: &str, which: &str) -> CmdResult {
    let project = load_project(ctx)?;
    let vehicle = &project
        .vehicles
        .get(name)
        .ok_or_else(|| {
            cli_error(
                format!("unknown vehicle `{name}`"),
                known(project.vehicles.keys()),
            )
        })?
        .value;
    let names: Vec<&str> = if which == "all" {
        default_manoeuvres(vehicle)
    } else if MANOEUVRES.contains(&which) {
        vec![which]
    } else {
        return Err(cli_error(
            format!("unknown manoeuvre `{which}`"),
            format!("manoeuvres: {}, all", MANOEUVRES.join(", ")),
        ));
    };
    let scene = SceneDef {
        name: "flat".into(),
        ground: GroundDef::default(),
        spawns: vec![SpawnPoint {
            name: "origin".into(),
            position: (0.0, 0.0),
            heading_deg: 0.0,
        }],
        docks: vec![],
        roads: None,
        traffic: None,
        parked: vec![],
        pedestrians: None,
        weather: None,
        time_of_day: None,
        scenery: None,
        terrain: None,
        grass: None,
        water: Vec::new(),
    };
    let make_app = || waypoint::make_app(project.disabled_plugins());
    let mut results = Vec::new();
    let mut all_ok = true;
    for m in names {
        let def = manoeuvre(m, name).expect("known manoeuvre");
        let input = ScenarioInput {
            name: m,
            scenario: &def,
            scene: &scene,
            vehicle,
            trailer: None,
            roads: None,
            scenery: None,
            traffic: Vec::new(),
            mission: None,
            mission_script: None,
            // Standard manoeuvres measure the vehicle alone, without mod code.
            wasm: &[],
        };
        let r = run_scenario(&input, &make_app).map_err(|d| vec![d])?;
        all_ok &= r.passed;
        let x = &r.metrics;
        let (label, value) = match m {
            "accel_0_50" => (
                "time to 50 km/h",
                x.time_to_50_kmh_s.map(|t| format!("{t:.2} s")),
            ),
            "accel_0_100" => (
                "time to 100 km/h",
                x.time_to_100_kmh_s.map(|t| format!("{t:.2} s")),
            ),
            "brake_60_0" | "brake_100_0" => (
                "stopping distance",
                x.braking_distance_m.map(|d| match x.mfdd_mps2 {
                    Some(a) => format!("{d:.1} m, MFDD {a:.2} m/s²"),
                    None => format!("{d:.1} m"),
                }),
            ),
            "turning_circle" => (
                "turning circle",
                x.turning_circle_m.map(|d| format!("{d:.1} m diameter")),
            ),
            _ => (
                "steady lateral",
                Some(format!(
                    "{:.3} g (peak {:.3} g)",
                    x.steady_lateral_g, x.max_lateral_g
                )),
            ),
        };
        let value = value.unwrap_or_else(|| "not reached".into());
        if !ctx.json {
            println!(
                "{m:<15} {label:<18} {value}{}",
                if r.passed { "" } else { "   ✗ see --json" }
            );
        }
        results.push(json!({ "manoeuvre": m, "result": value, "passed": r.passed, "report": r }));
    }
    if ctx.json {
        print_json(&json!({ "vehicle": name, "passed": all_ok, "manoeuvres": results }));
    }
    Ok(if all_ok { 0 } else { 1 })
}
