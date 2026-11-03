use crate::def::{Assertion, Driver, ScenarioDef};
use serde::Serialize;
use wp_app::App;
use wp_core::{Diagnostic, SimRng, SimTime};
use wp_ecs::Stage;
use wp_vehicle::{VehicleDef, VehicleInput};
use wp_world::{Name, Player, SceneDef};

/// Everything a scenario run needs, already loaded and validated.
pub struct ScenarioInput<'a> {
    pub name: &'a str,
    pub scenario: &'a ScenarioDef,
    pub scene: &'a SceneDef,
    pub vehicle: &'a VehicleDef,
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
    pub time_to_100_kmh_s: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub braking_distance_m: Option<f32>,
    pub final_position: [f32; 2],
    pub final_heading_deg: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScenarioReport {
    pub name: String,
    pub passed: bool,
    pub assertions: Vec<AssertionResult>,
    pub metrics: Metrics,
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
    distance_m: f32,
}

struct Run {
    samples: Vec<Sample>,
    /// World hash at every whole second.
    trace: Vec<String>,
    final_hash: String,
    nan_at: Option<f32>,
    metrics: Metrics,
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
        final_hash: run.final_hash,
        wall_ms: start.elapsed().as_millis(),
    })
}

fn simulate(input: &ScenarioInput, make_app: &dyn Fn() -> App) -> Result<Run, Diagnostic> {
    let s = input.scenario;
    let spawn = input.scene.spawn(&s.player.spawn).ok_or_else(|| {
        Diagnostic::error(
            "WP-S012",
            format!("spawn point `{}` is not in the scene", s.player.spawn),
        )
    })?;
    let mut app = make_app();
    app.world.insert_resource(SimRng::new(s.seed));
    wp_world::apply_scene(&mut app.world, input.scene);
    let (pos, heading) = wp_world::spawn_transform(spawn);
    let player = wp_vehicle::spawn_vehicle(&mut app.world, input.vehicle, pos, heading);
    if s.player.initial_kmh > 0.0 {
        wp_vehicle::set_rolling_speed(&mut app.world, player, s.player.initial_kmh / 3.6);
    }
    app.world.insert(player, Name(input.vehicle.name.clone()));
    app.world.insert(player, Player);

    let mut driver = DriverState::new(s.player.driver.clone());
    app.add_system(Stage::Agents, "scenario_driver", move |w| {
        let t = w.resource::<SimTime>().seconds() as f32;
        let dt = w.resource::<SimTime>().dt();
        let forward_kmh = wp_vehicle::telemetry(w, player)
            .map(|t| t.forward_kmh)
            .unwrap_or(0.0);
        let input = driver.next(t, dt, forward_kmh);
        if let Some(mut i) = w.get_mut::<VehicleInput>(player) {
            *i = input;
        }
    })
    .writes::<VehicleInput>();
    app.finish()?;

    let time = app.time();
    let ticks = time.ticks_for(s.duration_s);
    let mut samples = Vec::with_capacity(ticks as usize);
    let mut trace = Vec::new();
    let mut distance = 0.0f32;
    let mut last_pos = pos;
    let mut nan_at = None;
    for tick in 1..=ticks {
        let brake = app
            .world
            .get::<VehicleInput>(player)
            .map(|i| i.brake)
            .unwrap_or(0.0);
        app.tick();
        let t = tick as f32 / time.tick_hz as f32;
        let tel = wp_vehicle::telemetry(&app.world, player).expect("player has telemetry");
        let p = glam_vec(tel.position);
        if !(tel.speed_kmh.is_finite() && p.is_finite() && tel.lateral_g.is_finite()) {
            nan_at = Some(t);
            break;
        }
        distance += (p - last_pos).length();
        last_pos = p;
        samples.push(Sample {
            t,
            speed_kmh: tel.speed_kmh,
            lateral_g: tel.lateral_g,
            brake,
            distance_m: distance,
        });
        if tick % time.tick_hz as u64 == 0 {
            trace.push(app.world.state_hash());
        }
    }

    let tel = wp_vehicle::telemetry(&app.world, player).expect("player has telemetry");
    let mut metrics = Metrics {
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
        time_to_100_kmh_s: samples.iter().find(|s| s.speed_kmh >= 100.0).map(|s| s.t),
        braking_distance_m: None,
        final_position: tel.position,
        final_heading_deg: tel.heading_deg,
    };
    metrics.braking_distance_m = braking(&samples).and_then(|b| b.distance);
    Ok(Run {
        samples,
        trace,
        final_hash: app.world.state_hash(),
        nan_at,
        metrics,
    })
}

fn glam_vec(p: [f32; 2]) -> wp_core::glam::Vec2 {
    wp_core::glam::Vec2::from_array(p)
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
}

/// From the first tick with brake ≥ 0.5 while moving, to standstill.
fn braking(samples: &[Sample]) -> Option<Braking> {
    let start = samples
        .iter()
        .position(|s| s.brake >= 0.5 && s.speed_kmh > 1.0)?;
    let s0 = samples[start];
    let stop = samples[start..].iter().find(|s| s.speed_kmh < 0.5);
    Some(Braking {
        from_kmh: s0.speed_kmh,
        distance: stop.map(|s| s.distance_m - s0.distance_m),
    })
}

fn evaluate(a: &Assertion, run: &Run) -> (bool, String) {
    let samples = &run.samples;
    if let Some(t) = run.nan_at
        && !matches!(a, Assertion::Deterministic)
    {
        return (false, format!("simulation produced NaN at t = {t:.2} s"));
    }
    match a {
        Assertion::ReachSpeed { kmh, within_s } => {
            match samples.iter().find(|s| s.speed_kmh >= *kmh) {
                Some(s) if s.t <= *within_s => (true, format!("reached at {:.2} s", s.t)),
                Some(s) => (false, format!("reached only at {:.2} s", s.t)),
                None => (
                    false,
                    format!("top speed was {:.1} km/h", run.metrics.max_speed_kmh),
                ),
            }
        }
        Assertion::BrakingDistance { max_m } => match braking(samples) {
            None => (
                false,
                "the brake was never fully applied while moving".into(),
            ),
            Some(Braking {
                distance: None,
                from_kmh,
            }) => (
                false,
                format!("braked from {from_kmh:.1} km/h but never stopped"),
            ),
            Some(Braking {
                distance: Some(d),
                from_kmh,
            }) => (d <= *max_m, format!("{d:.1} m from {from_kmh:.1} km/h")),
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
                    format!(
                        "still moving at the end ({:.1} km/h)",
                        run.metrics.final_speed_kmh
                    ),
                ),
            }
        }
        Assertion::MaxSpeed(k) => {
            let m = run.metrics.max_speed_kmh;
            (m <= *k, format!("top speed {m:.1} km/h"))
        }
        Assertion::MaxLateralG(g) => {
            let m = run.metrics.max_lateral_g;
            (m <= *g, format!("peak {m:.3} g"))
        }
        Assertion::SteadyLateralG { min, max } => {
            if samples.is_empty() {
                return (false, "no samples".into());
            }
            let mean = run.metrics.steady_lateral_g;
            (
                mean >= *min && mean <= *max,
                format!("mean {mean:.3} g over the second half"),
            )
        }
        Assertion::NoNaN => (run.nan_at.is_none(), "all values finite".into()),
        Assertion::Deterministic => unreachable!("handled by run_scenario"),
    }
}

/// Produces driver inputs each tick.
struct DriverState {
    driver: Driver,
    integral: f32,
}

impl DriverState {
    fn new(driver: Driver) -> Self {
        Self {
            driver,
            integral: 0.0,
        }
    }

    fn next(&mut self, t: f32, dt: f32, forward_kmh: f32) -> VehicleInput {
        match &self.driver {
            Driver::Script(keys) => keys
                .iter()
                .rev()
                .find(|k| k.at_s <= t)
                .map(|k| VehicleInput {
                    throttle: k.throttle,
                    brake: k.brake,
                    steer: k.steer,
                })
                .unwrap_or_default(),
            Driver::Cruise { kmh, steer } => {
                // PI speed controller on km/h error.
                let err = kmh - forward_kmh;
                self.integral = (self.integral + 0.02 * err * dt).clamp(0.0, 0.6);
                let throttle = (0.08 * err + self.integral).clamp(0.0, 1.0);
                let brake = if err < -3.0 {
                    (-0.05 * err).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                VehicleInput {
                    throttle,
                    brake,
                    steer: *steer,
                }
            }
        }
    }
}
