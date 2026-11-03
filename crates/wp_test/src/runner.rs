use crate::def::{Assertion, Driver, ScenarioDef};
use crate::drivers::DriverState;
use serde::Serialize;
use std::collections::BTreeMap;
use wp_app::App;
use wp_core::glam::Vec2;
use wp_core::{Diagnostic, SimRng, SimTime};
use wp_ecs::{Entity, Events, Stage};
use wp_physics::Pose;
use wp_vehicle::{VehicleDef, VehicleEvent, VehicleInput};
use wp_world::{Name, Player, SceneDef};

/// Everything a scenario run needs, already loaded and validated.
pub struct ScenarioInput<'a> {
    pub name: &'a str,
    pub scenario: &'a ScenarioDef,
    pub scene: &'a SceneDef,
    pub vehicle: &'a VehicleDef,
    /// Semi-trailer to couple to the player, if the scenario names one.
    pub trailer: Option<&'a wp_vehicle::TrailerDef>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssertionResult {
    pub assertion: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    pub sim_seconds: f32,
    pub ticks: u64,
    pub physics_hz: u32,
    pub max_speed_kmh: f32,
    pub final_speed_kmh: f32,
    pub distance_m: f32,
    pub max_lateral_g: f32,
    /// Mean |lateral g| over the second half of the run.
    pub steady_lateral_g: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_50_kmh_s: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_100_kmh_s: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub braking_distance_m: Option<f32>,
    /// Mean fully developed deceleration as defined in UN ECE R13, m/s².
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfdd_mps2: Option<f32>,
    /// Diameter of the outer front wheel's path, m (when the run circles).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turning_circle_m: Option<f32>,
    /// How often each vehicle event happened to the player.
    pub events: BTreeMap<String, u32>,
    pub final_position: [f32; 2],
    pub final_heading_deg: f32,
    /// Centre of the towed trailer's rear edge at the end, if towing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailer_rear: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailer_heading_deg: Option<f32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScenarioReport {
    pub name: String,
    pub passed: bool,
    pub assertions: Vec<AssertionResult>,
    pub metrics: Metrics,
    /// The player's first 50 events with their times, for debugging.
    pub event_log: Vec<serde_json::Value>,
    /// Hash of the full world state at the end of the run.
    pub final_hash: String,
    /// Real time the run took, ms. Informational only; never asserted on.
    pub wall_ms: u128,
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    t: f32,
    speed_kmh: f32,
    lateral_g: f32,
    brake: f32,
    steer: f32,
    distance_m: f32,
    pose: Pose,
}

struct Run {
    samples: Vec<Sample>,
    docks: Vec<wp_world::DockDef>,
    /// World hash at every whole second.
    trace: Vec<String>,
    final_hash: String,
    nan_at: Option<f32>,
    metrics: Metrics,
    event_log: Vec<serde_json::Value>,
}

/// Run a scenario headless. `make_app` returns an app with plugins added but
/// not finished (for example `DefaultPlugins` minus any disabled ones).
pub fn run_scenario(
    input: &ScenarioInput,
    make_app: &dyn Fn() -> App,
) -> Result<ScenarioReport, Diagnostic> {
    let start = std::time::Instant::now();
    let run = simulate(input, make_app)?;
    let mut results: Vec<AssertionResult> = Vec::new();
    for a in &input.scenario.assert {
        let (passed, detail) = match a {
            Assertion::Deterministic => {
                let again = simulate(input, make_app)?;
                match run.trace.iter().zip(&again.trace).position(|(x, y)| x != y) {
                    None if run.final_hash == again.final_hash => (
                        true,
                        format!("identical hashes over {} checkpoints", run.trace.len()),
                    ),
                    None => (false, "final state hash differs between two runs".into()),
                    Some(i) => (false, format!("runs diverged by t = {} s", i + 1)),
                }
            }
            other => evaluate(other, &run),
        };
        results.push(AssertionResult {
            assertion: a.describe(),
            passed,
            detail,
        });
    }
    Ok(ScenarioReport {
        name: input.name.to_string(),
        passed: results.iter().all(|r| r.passed),
        assertions: results,
        metrics: run.metrics,
        event_log: run.event_log,
        final_hash: run.final_hash,
        wall_ms: start.elapsed().as_millis(),
    })
}

/// Build a finished app for a scenario: scene applied, player spawned and its
/// driver installed, at t = 0. Returns the app and the player entity. Use this
/// to inspect or screenshot a scenario at any time (`wp shot --scenario`).
pub fn prepare(
    input: &ScenarioInput,
    make_app: &dyn Fn() -> App,
) -> Result<(App, Entity), Diagnostic> {
    let s = input.scenario;
    let spawn = input.scene.spawn(&s.player.spawn).ok_or_else(|| {
        Diagnostic::error(
            "WP-S012",
            format!("spawn point `{}` is not in the scene", s.player.spawn),
        )
    })?;
    let mut app = make_app();
    app.world
        .insert_reflected_resource("SimRng", SimRng::new(s.seed));
    wp_world::apply_scene(&mut app.world, input.scene);
    let (pos, heading) = wp_world::spawn_transform(spawn);
    let player = wp_vehicle::spawn_vehicle(&mut app.world, input.vehicle, pos, heading);
    if s.player.initial_kmh > 0.0 {
        wp_vehicle::set_rolling_speed(&mut app.world, player, s.player.initial_kmh / 3.6);
    }
    app.world.insert(player, Name(input.vehicle.name.clone()));
    app.world.insert(player, Player);
    if let Some(tdef) = input.trailer {
        let t = wp_vehicle::spawn_trailer(&mut app.world, tdef, player).ok_or_else(|| {
            Diagnostic::error(
                "WP-S019",
                format!(
                    "vehicle `{}` has no fifth wheel to couple `{}` to",
                    input.vehicle.name, tdef.name
                ),
            )
            .help("add hitch: Some((ahead_of_rear_axle_m: 0.5)) to the vehicle")
        })?;
        app.world.insert(t, Name(tdef.name.clone()));
    }

    let dock = match &s.player.driver {
        Driver::ReverseToDock { dock, .. } => {
            Some(input.scene.dock(dock).cloned().ok_or_else(|| {
                Diagnostic::error("WP-S020", format!("dock `{dock}` is not in the scene"))
            })?)
        }
        _ => None,
    };
    let mut driver = DriverState::new(s.player.driver.clone(), dock);
    app.add_system(Stage::Agents, "scenario_driver", move |w| {
        let t = w.resource::<SimTime>().seconds() as f32;
        let dt = w.resource::<SimTime>().dt();
        let input = driver.next(t, dt, w, player);
        if let Some(mut i) = w.get_mut::<VehicleInput>(player) {
            *i = input;
        }
    })
    .writes::<VehicleInput>();
    app.finish()?;
    Ok((app, player))
}

fn simulate(input: &ScenarioInput, make_app: &dyn Fn() -> App) -> Result<Run, Diagnostic> {
    let s = input.scenario;
    let (mut app, player) = prepare(input, make_app)?;
    let player_id = player.to_string();
    let start_pos = app
        .world
        .get::<Pose>(player)
        .map(|p| p.position)
        .unwrap_or_default();

    let time = app.time();
    let ticks = time.ticks_for(s.duration_s);
    let mut samples = Vec::with_capacity(ticks as usize);
    let mut trace = Vec::new();
    let mut distance = 0.0f32;
    let mut last_pos = start_pos;
    let mut nan_at = None;
    let mut event_counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut event_log = Vec::new();
    for tick in 1..=ticks {
        let input_now = app
            .world
            .get::<VehicleInput>(player)
            .map(|i| *i)
            .unwrap_or_default();
        app.tick();
        let t = tick as f32 / time.tick_hz as f32;
        if let Some(events) = app.world.get_resource::<Events<VehicleEvent>>() {
            for e in events.iter().filter(|e| event_entity(e) == player_id) {
                *event_counts.entry(e.kind().to_string()).or_default() += 1;
                if event_log.len() < 50 {
                    let mut v = serde_json::to_value(e).unwrap();
                    v["t"] = serde_json::json!(t);
                    event_log.push(v);
                }
            }
        }
        let tel = wp_vehicle::telemetry(&app.world, player).expect("player has telemetry");
        let pose = *app.world.get::<Pose>(player).expect("player has a pose");
        if !(tel.speed_kmh.is_finite() && pose.position.is_finite() && tel.lateral_g.is_finite()) {
            nan_at = Some(t);
            break;
        }
        distance += (pose.position - last_pos).length();
        last_pos = pose.position;
        samples.push(Sample {
            t,
            speed_kmh: tel.speed_kmh,
            lateral_g: tel.lateral_g,
            brake: input_now.brake,
            steer: input_now.steer,
            distance_m: distance,
            pose,
        });
        if tick % time.tick_hz as u64 == 0 {
            trace.push(app.world.state_hash());
        }
    }

    let tel = wp_vehicle::telemetry(&app.world, player).expect("player has telemetry");
    let rig = crate::drivers::RigGeometry::read(&app.world, player);
    let first_reach = |kmh: f32| samples.iter().find(|s| s.speed_kmh >= kmh).map(|s| s.t);
    let braking = braking(&samples);
    let metrics = Metrics {
        sim_seconds: samples.last().map(|s| s.t).unwrap_or(0.0),
        ticks: samples.len() as u64,
        physics_hz: time.physics_hz(),
        max_speed_kmh: samples.iter().map(|s| s.speed_kmh).fold(0.0, f32::max),
        final_speed_kmh: tel.speed_kmh,
        distance_m: distance,
        max_lateral_g: samples
            .iter()
            .map(|s| s.lateral_g.abs())
            .fold(0.0, f32::max),
        steady_lateral_g: steady_lateral_g(&samples),
        time_to_50_kmh_s: first_reach(50.0),
        time_to_100_kmh_s: first_reach(100.0),
        braking_distance_m: braking.as_ref().and_then(|b| b.distance),
        mfdd_mps2: braking.as_ref().and_then(|b| b.mfdd),
        turning_circle_m: turning_circle(&samples, input.vehicle),
        events: event_counts,
        final_position: tel.position,
        final_heading_deg: tel.heading_deg,
        trailer_rear: rig.map(|g| g.trailer_rear.to_array()),
        trailer_heading_deg: rig.map(|g| g.trailer_heading.to_degrees()),
    };
    Ok(Run {
        samples,
        docks: input.scene.docks.clone(),
        trace,
        final_hash: app.world.state_hash(),
        nan_at,
        metrics,
        event_log,
    })
}

fn event_entity(e: &VehicleEvent) -> &str {
    e.entity()
}

fn steady_lateral_g(samples: &[Sample]) -> f32 {
    let half = &samples[samples.len() / 2..];
    if half.is_empty() {
        return 0.0;
    }
    half.iter().map(|s| s.lateral_g.abs()).sum::<f32>() / half.len() as f32
}

struct Braking {
    from_kmh: f32,
    distance: Option<f32>,
    mfdd: Option<f32>,
}

/// From the first tick with brake ≥ 0.5 while moving, to standstill.
fn braking(samples: &[Sample]) -> Option<Braking> {
    let start = samples
        .iter()
        .position(|s| s.brake >= 0.5 && s.speed_kmh > 1.0)?;
    // Distance is measured from the moment the pedal went down (the sample before).
    let s0 = if start > 0 {
        samples[start - 1]
    } else {
        samples[start]
    };
    let v0 = s0.speed_kmh;
    let after = &samples[start..];
    let stop = after.iter().find(|s| s.speed_kmh < 0.5);
    // UN ECE R13 Annex 4: dm = (vb² − ve²) / (25.92 (se − sb)), with vb = 0.8 v0 and
    // ve = 0.1 v0 in km/h and the distances in metres where those speeds are reached.
    let at = |frac: f32| after.iter().find(|s| s.speed_kmh <= frac * v0);
    let mfdd = match (at(0.8), at(0.1)) {
        (Some(b), Some(e)) if e.distance_m > b.distance_m => {
            let (vb, ve) = (0.8 * v0, 0.1 * v0);
            Some((vb * vb - ve * ve) / (25.92 * (e.distance_m - b.distance_m)))
        }
        _ => None,
    };
    Some(Braking {
        from_kmh: v0,
        distance: stop.map(|s| s.distance_m - s0.distance_m),
        mfdd,
    })
}

/// Diameter of the path of the outer front wheel, from samples after the first
/// third of the run, if the vehicle turned through at least a full circle there.
fn turning_circle(samples: &[Sample], def: &VehicleDef) -> Option<f32> {
    let part = &samples[samples.len() / 3..];
    let first = part.first()?;
    let last = part.last()?;
    let turned = part
        .windows(2)
        .map(|w| angle_diff(w[1].pose.heading, w[0].pose.heading))
        .sum::<f32>();
    if turned.abs() < std::f32::consts::TAU || first.steer == 0.0 {
        return None;
    }
    let _ = last;
    // Outer front wheel: right side when steering left, left side when steering right.
    let side = if first.steer > 0.0 { -1.0 } else { 1.0 };
    let local = Vec2::new(def.cg_to_front_axle_m, side * def.track_width_m / 2.0);
    let points: Vec<Vec2> = part
        .iter()
        .step_by(4)
        .map(|s| s.pose.position + s.pose.to_world(local))
        .collect();
    let mut best = 0.0f32;
    for (i, a) in points.iter().enumerate() {
        for b in &points[i + 1..] {
            best = best.max(a.distance(*b));
        }
    }
    Some(best)
}

fn angle_diff(a: f32, b: f32) -> f32 {
    (a - b + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

fn evaluate(a: &Assertion, run: &Run) -> (bool, String) {
    let samples = &run.samples;
    let m = &run.metrics;
    if let Some(t) = run.nan_at
        && !matches!(a, Assertion::Deterministic)
    {
        return (false, format!("simulation produced NaN at t = {t:.2} s"));
    }
    match a {
        Assertion::ReachSpeed {
            kmh,
            within_s,
            not_before_s,
        } => match samples.iter().find(|s| s.speed_kmh >= *kmh) {
            Some(s) if s.t <= *within_s && s.t >= *not_before_s => {
                (true, format!("reached at {:.2} s", s.t))
            }
            Some(s) if s.t < *not_before_s => (
                false,
                format!(
                    "reached at {:.2} s, sooner than {not_before_s} s (implausibly quick)",
                    s.t
                ),
            ),
            Some(s) => (false, format!("reached only at {:.2} s", s.t)),
            None => (false, format!("top speed was {:.1} km/h", m.max_speed_kmh)),
        },
        Assertion::BrakingDistance { max_m } => match braking(samples) {
            None => (
                false,
                "the brake was never fully applied while moving".into(),
            ),
            Some(Braking {
                distance: None,
                from_kmh,
                ..
            }) => (
                false,
                format!("braked from {from_kmh:.1} km/h but never stopped"),
            ),
            Some(Braking {
                distance: Some(d),
                from_kmh,
                ..
            }) => (d <= *max_m, format!("{d:.1} m from {from_kmh:.1} km/h")),
        },
        Assertion::Mfdd { min_mps2 } => match m.mfdd_mps2 {
            Some(d) => (
                d >= *min_mps2,
                format!("{d:.2} m/s² (ECE R13 mean fully developed deceleration)"),
            ),
            None => (false, "no complete braking from speed to 10% of it".into()),
        },
        Assertion::TurningCircle { max_m } => match m.turning_circle_m {
            Some(d) => (
                d <= *max_m,
                format!("{d:.1} m diameter (outer front wheel)"),
            ),
            None => (
                false,
                "the vehicle did not complete a full circle while steering".into(),
            ),
        },
        Assertion::StoppedBy { s } => {
            let stopped_from = samples
                .iter()
                .rposition(|x| x.speed_kmh >= 0.5)
                .map(|i| i + 1)
                .unwrap_or(0);
            match samples.get(stopped_from) {
                Some(x) if x.t <= *s => (true, format!("stopped at {:.2} s", x.t)),
                Some(x) => (false, format!("stopped only at {:.2} s", x.t)),
                None => (
                    false,
                    format!("still moving at the end ({:.1} km/h)", m.final_speed_kmh),
                ),
            }
        }
        Assertion::MaxSpeed(k) => (
            m.max_speed_kmh <= *k,
            format!("top speed {:.1} km/h", m.max_speed_kmh),
        ),
        Assertion::MaxLateralG(g) => (
            m.max_lateral_g <= *g,
            format!("peak {:.3} g", m.max_lateral_g),
        ),
        Assertion::SteadyLateralG { min, max } => {
            if samples.is_empty() {
                return (false, "no samples".into());
            }
            let mean = m.steady_lateral_g;
            (
                mean >= *min && mean <= *max,
                format!("mean {mean:.3} g over the second half"),
            )
        }
        Assertion::NoEvent(kind) => match m.events.get(kind) {
            None => (true, format!("no {kind} events")),
            Some(n) => {
                let first = run
                    .event_log
                    .iter()
                    .find(|e| e["kind"] == kind.as_str())
                    .and_then(|e| e["t"].as_f64());
                (
                    false,
                    format!(
                        "{n} {kind} event(s), first at {:.2} s",
                        first.unwrap_or(f64::NAN)
                    ),
                )
            }
        },
        Assertion::ExpectEvent(kind) => match m.events.get(kind) {
            Some(n) => (true, format!("{n} {kind} event(s)")),
            None => (false, format!("no {kind} event happened")),
        },
        Assertion::Docked {
            dock,
            max_offset_m,
            max_angle_deg,
        } => {
            let Some(d) = run.docks.iter().find(|d| &d.name == dock) else {
                return (false, format!("dock `{dock}` is not in the scene"));
            };
            let (Some(rear), Some(heading)) = (m.trailer_rear, m.trailer_heading_deg) else {
                return (false, "the player is not towing a trailer".into());
            };
            let offset = Vec2::from_array(rear).distance(d.point());
            let angle = (heading - d.heading_deg + 180.0).rem_euclid(360.0) - 180.0;
            let stopped = m.final_speed_kmh < 0.5;
            let ok = offset <= *max_offset_m && angle.abs() <= *max_angle_deg && stopped;
            (
                ok,
                format!(
                    "rear {offset:.2} m from the dock point, {:.1}° off its heading, {:.1} km/h",
                    angle.abs(),
                    m.final_speed_kmh
                ),
            )
        }
        Assertion::NoNaN => (run.nan_at.is_none(), "all values finite".into()),
        Assertion::Deterministic => unreachable!("handled by run_scenario"),
    }
}
