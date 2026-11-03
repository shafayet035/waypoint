use crate::{CmdResult, Ctx, catalog};
use serde_json::json;
use std::path::Path;
use waypoint::app::App;
use waypoint::core::{Diagnostic, SimRng};
use waypoint::project::{GameConfig, Project};
use waypoint::test::{
    Assertion, Driver, InputKey, PlayerDef, ScenarioDef, ScenarioInput, ScenarioReport,
    run_scenario,
};
use waypoint::vehicle::{VehicleDef, VehicleInput};
use waypoint::world::{GroundDef, Name, Player, SceneDef, SpawnPoint};
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

// ---- wp new ---------------------------------------------------------------

const TEMPLATE_SANDBOX: &[(&str, &str)] = &[
    (
        "game.toml",
        include_str!("../../../examples/sandbox/game.toml"),
    ),
    (
        "vehicles/hatchback.vehicle.ron",
        include_str!("../../../examples/sandbox/vehicles/hatchback.vehicle.ron"),
    ),
    (
        "vehicles/tractor.vehicle.ron",
        include_str!("../../../examples/sandbox/vehicles/tractor.vehicle.ron"),
    ),
    (
        "trailers/semi_trailer.trailer.ron",
        include_str!("../../../examples/sandbox/trailers/semi_trailer.trailer.ron"),
    ),
    (
        "tests/truck_brake_60.scenario.ron",
        include_str!("../../../examples/sandbox/tests/truck_brake_60.scenario.ron"),
    ),
    (
        "vehicles/city_bus.vehicle.ron",
        include_str!("../../../examples/sandbox/vehicles/city_bus.vehicle.ron"),
    ),
    (
        "scenes/flat.scene.ron",
        include_str!("../../../examples/sandbox/scenes/flat.scene.ron"),
    ),
    (
        "tests/accelerate_and_brake.scenario.ron",
        include_str!("../../../examples/sandbox/tests/accelerate_and_brake.scenario.ron"),
    ),
    (
        "tests/skidpad.scenario.ron",
        include_str!("../../../examples/sandbox/tests/skidpad.scenario.ron"),
    ),
    (
        "tests/car_brake_100.scenario.ron",
        include_str!("../../../examples/sandbox/tests/car_brake_100.scenario.ron"),
    ),
    (
        "tests/bus_brake_60.scenario.ron",
        include_str!("../../../examples/sandbox/tests/bus_brake_60.scenario.ron"),
    ),
    (
        "tests/bus_acceleration.scenario.ron",
        include_str!("../../../examples/sandbox/tests/bus_acceleration.scenario.ron"),
    ),
    (
        "tests/bus_turning_circle.scenario.ron",
        include_str!("../../../examples/sandbox/tests/bus_turning_circle.scenario.ron"),
    ),
    (
        "AGENTS.md",
        include_str!("../../../examples/sandbox/AGENTS.md"),
    ),
];

pub fn new(ctx: &Ctx, dest: &Path, template: &str) -> CmdResult {
    if template != "sandbox" {
        return Err(cli_error(
            format!("unknown template `{template}`"),
            "available templates: sandbox",
        ));
    }
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
    for (rel, content) in TEMPLATE_SANDBOX {
        let path = dest.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| io_error(&path, e))?;
        let content = if *rel == "game.toml" {
            content.replacen("name = \"sandbox\"", &format!("name = \"{game_name}\""), 1)
        } else {
            content.to_string()
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
        other => {
            return Err(cli_error(
                format!("unknown schema type `{other}`"),
                "types: game, vehicle, trailer, scene, scenario",
            ));
        }
    };
    println!("{text}");
    Ok(0)
}

// ---- wp docs --------------------------------------------------------------

const DOCS_CLI: &str = "\
wp — Waypoint command line. Every command takes --json and --project <dir>.

  wp new <dir>                 create a project from the sandbox template
  wp check                     validate all files (schemas + cross-references)
  wp schema <type>             JSON Schema: game | vehicle | scene | scenario
  wp docs [topic]              offline docs: cli | errors | errors/<CODE> | scenario | vehicle
  wp run [drive opts]          drive in a window: W/S throttle-brake, A/D steer, C camera, R reset, P pause
  wp run --headless [opts]     simulate without a window; prints the final state (+ --trace, --seconds, --ticks)
  wp run --control <port>      also accept control-protocol requests on 127.0.0.1:<port> (0 = any free port)
  wp shot --at 12.5s [opts]    render one frame to PNG, headless: --camera chase|top|side|showcase --out f.png
  wp serve [drive opts]        control protocol (JSON-RPC 2.0) on stdin/stdout, one request per line
  wp mcp                       Model Context Protocol server on stdin/stdout (tools, resources, screenshots)
  wp test [filter]             run scenario tests; exit 1 if any fail
  wp vehicle test <name> [--manoeuvre accel_0_100|brake_100_0|skidpad|all]
  wp systems                   resolved system schedule
  wp plugins                   loaded plugins in load order

Drive options (run and shot): --scenario NAME, or --scene PATH --spawn NAME --vehicle NAME
  --throttle 0..1 --brake 0..1 --steer -1..1 --seed N

Agent loop: edit a file → wp check --json → wp test --json → on failure, wp shot --scenario <name> --at <t>
and look at the image → fix. For live work, `wp serve` or `wp mcp`, then call rpc.methods.

Control protocol methods: rpc.methods, sim.status, sim.step, sim.pause, sim.resume, sim.snapshot,
sim.restore, world.entities, world.get, world.set, world.find, world.spawn, world.despawn,
world.resources, input.set, vehicle.telemetry, events.poll, render.screenshot.
";

const DOCS_SCENARIO: &str = "\
Scenario files: tests/<name>.scenario.ron  (full schema: wp schema scenario)

  (
    scene: \"scenes/flat.scene.ron\",
    seed: 7,
    player: (vehicle: \"hatchback\", spawn: \"start\", initial_kmh: 0.0, driver: <Driver>),
    duration_s: 20.0,
    assert: [<Assertion>, ...],
  )

Drivers
  Script([(at_s: 0.0, throttle: 1.0), (at_s: 9.0, brake: 1.0, steer: 0.0)])   keys hold until the next
  Cruise(kmh: 40.0, steer: 0.25)                                              speed controller + fixed steer

Assertions
  ReachSpeed(kmh: 50.0, within_s: 15.0, not_before_s: 8.0)   (not_before_s is optional)
  BrakingDistance(max_m: 36.7)    Mfdd(min_mps2: 5.0)    StoppedBy(s: 14.0)    TurningCircle(max_m: 25.0)
  MaxSpeed(130.0)   MaxLateralG(0.35)   SteadyLateralG(min: 0.4, max: 1.05)
  NoEvent(\"WheelLift\")   ExpectEvent(\"AbsEngaged\")   NoNaN   Deterministic

Events: GearShift, AbsEngaged, AbsReleased, WheelLift, WheelLanded, LowAirPressure, RolledOver.
Regulation limits used in the sandbox: UN ECE R13 Type-0 braking — buses (M3) from 60 km/h
≤ 0.15v + v²/130 = 36.7 m and MFDD ≥ 5.0 m/s²; cars (M1, R13-H) from 100 km/h ≤ 70 m and
MFDD ≥ 6.43 m/s². EU turning circle for buses: within 25 m.

Runs are headless at 60 Hz gameplay / 480 Hz physics, and bit-identical for the same build and seed.
";

const DOCS_VEHICLE: &str = "\
Vehicle files: vehicles/<name>.vehicle.ron  (full schema with units: wp schema vehicle)

Units are in field names (mass_kg, radius_m, drag_area_m2). Model: brush tyres with relaxation
length and sliding friction, body roll and pitch on springs, tipping about the outer wheels,
torque-curve engine with optional speed governor, automatic gearbox, open differential, brakes
(hydraulic, or air with reservoir, compressor and chamber lag) with optional ABS, aerodynamic drag.
Optional sections: suspension, body (with style: Car | Bus | TruckCab), brakes.air, brakes.abs,
engine.governed_speed_kmh, gearbox.reverse_ratio, hitch (fifth wheel for semi-trailers).
Semi-trailers live in trailers/<name>.trailer.ron (`wp schema trailer`). Couple one in a scenario
with player.trailer: Some(\"semi_trailer\"), or `wp run --vehicle tractor --trailer semi_trailer`.
Check behaviour with `wp vehicle test <name>` (100 km/h tests are skipped for governed vehicles):
  accel_0_50 / accel_0_100   time from rest
  brake_60_0 / brake_100_0   stopping distance and ECE R13 MFDD
  skidpad                    steady lateral g at 40 km/h with fixed steering
  turning_circle             diameter of the outer front wheel's path on full lock
";

pub fn docs(ctx: &Ctx, topic: Option<&str>) -> CmdResult {
    let text = match topic {
        None => "topics: cli, errors, errors/<CODE>, scenario, vehicle\nexample: wp docs errors/WP-V012\n".to_string(),
        Some("cli") => DOCS_CLI.to_string(),
        Some("scenario") => DOCS_SCENARIO.to_string(),
        Some("vehicle") => DOCS_VEHICLE.to_string(),
        Some("errors") => {
            if ctx.json {
                let list: Vec<_> = catalog::ERRORS
                    .iter()
                    .map(|e| json!({ "code": e.code, "title": e.title, "fix": e.fix }))
                    .collect();
                print_json(&json!(list));
                return Ok(0);
            }
            catalog::ERRORS.iter().map(|e| format!("{}  {}\n", e.code, e.title)).collect()
        }
        Some(t) if t.starts_with("errors/") => {
            let code = &t["errors/".len()..];
            let Some(e) = catalog::find(code) else {
                return Err(cli_error(format!("unknown error code `{code}`"), "list all codes with `wp docs errors`"));
            };
            if ctx.json {
                print_json(&json!({ "code": e.code, "title": e.title, "fix": e.fix }));
                return Ok(0);
            }
            format!("{}: {}\n\nHow to fix: {}\n", e.code, e.title, e.fix)
        }
        Some(other) => {
            return Err(cli_error(format!("unknown docs topic `{other}`"), "topics: cli, errors, errors/<CODE>, scenario, vehicle"));
        }
    };
    if ctx.json {
        print_json(&json!({ "topic": topic, "text": text }));
    } else {
        print!("{text}");
    }
    Ok(0)
}

// ---- wp run / wp shot --------------------------------------------------------

/// What to drive: shared by `wp run` and `wp shot`.
#[derive(clap::Args, Clone, Default)]
pub struct DriveArgs {
    /// Use a scenario's scene, vehicle, spawn and driver instead of constant inputs.
    #[arg(long)]
    pub(crate) scenario: Option<String>,
    /// Scene path; defaults to game.toml start_scene.
    #[arg(long)]
    pub(crate) scene: Option<String>,
    /// Spawn point; defaults to the scene's first.
    #[arg(long)]
    pub(crate) spawn: Option<String>,
    /// Vehicle name; defaults to the first vehicle.
    #[arg(long)]
    pub(crate) vehicle: Option<String>,
    #[arg(long, default_value_t = 0.0)]
    pub(crate) throttle: f32,
    #[arg(long, default_value_t = 0.0)]
    pub(crate) brake: f32,
    /// -1 (full right) to 1 (full left).
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub(crate) steer: f32,
    #[arg(long, default_value_t = 0)]
    pub(crate) seed: u64,
    /// Couple this semi-trailer to the vehicle (needs a tractor unit).
    #[arg(long)]
    pub(crate) trailer: Option<String>,
    /// Start in reverse gear.
    #[arg(long)]
    pub(crate) reverse: bool,
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// Run without a window: simulate as fast as possible and print the result.
    #[arg(long)]
    headless: bool,
    /// Simulated seconds for --headless (ignored if --ticks is set).
    #[arg(long, default_value_t = 10.0)]
    seconds: f32,
    /// Exact number of 60 Hz ticks to run headless.
    #[arg(long)]
    ticks: Option<u64>,
    /// Print telemetry every simulated second (headless).
    #[arg(long)]
    trace: bool,
    /// Starting camera in a window: chase, top, side, showcase.
    #[arg(long, default_value = "chase")]
    camera: String,
    /// Close the window after this many real seconds (for smoke tests).
    #[arg(long)]
    exit_after: Option<f32>,
    /// Accept control-protocol requests on 127.0.0.1:<port> while the window runs (0 = any free port).
    #[arg(long)]
    control: Option<u16>,
    #[command(flatten)]
    drive: DriveArgs,
}

#[derive(clap::Args)]
pub struct ServeArgs {
    #[command(flatten)]
    pub(crate) drive: DriveArgs,
}

#[derive(clap::Args)]
pub struct ShotArgs {
    /// Simulated time of the frame, e.g. 12.5 or 12.5s.
    #[arg(long, default_value = "0")]
    at: String,
    /// chase, top, side or showcase.
    #[arg(long, default_value = "chase")]
    camera: String,
    #[arg(long, default_value_t = 1280)]
    width: u32,
    #[arg(long, default_value_t = 720)]
    height: u32,
    /// Output PNG path.
    #[arg(long, default_value = "shot.png")]
    out: std::path::PathBuf,
    #[command(flatten)]
    drive: DriveArgs,
}

/// A ready-to-tick app with the player spawned, plus a label for messages.
struct Drive {
    app: App,
    player: waypoint::ecs::Entity,
    label: String,
}

fn setup_drive(ctx: &Ctx, project: &Project, args: &DriveArgs) -> Result<Drive, Vec<Diagnostic>> {
    let make_app = || waypoint::make_app(project.disabled_plugins());
    if let Some(name) = &args.scenario {
        let sc = project.scenarios.get(name).ok_or_else(|| {
            cli_error(
                format!("unknown scenario `{name}`"),
                known(project.scenarios.keys()),
            )
        })?;
        let _ = sc;
        let input = project.scenario_input(name).ok_or_else(|| {
            cli_error(format!("scenario `{name}` is incomplete"), "run `wp check`")
        })?;
        let (app, player) = waypoint::test::prepare(&input, &make_app).map_err(|d| vec![d])?;
        return Ok(Drive {
            app,
            player,
            label: format!("scenario `{name}`"),
        });
    }
    let _ = ctx;
    let scene_path = args
        .scene
        .clone()
        .unwrap_or_else(|| project.game.value.start_scene.clone());
    let scene = &project
        .scenes
        .get(&scene_path)
        .ok_or_else(|| {
            cli_error(
                format!("unknown scene `{scene_path}`"),
                known(project.scenes.keys()),
            )
        })?
        .value;
    let vehicle = match &args.vehicle {
        Some(v) => project.vehicles.get(v),
        None => project.vehicles.values().next(),
    }
    .ok_or_else(|| cli_error("vehicle not found", known(project.vehicles.keys())))?;
    let spawn = match &args.spawn {
        Some(s) => scene.spawn(s),
        None => scene.spawns.first(),
    }
    .ok_or_else(|| {
        cli_error(
            "spawn point not found",
            known(scene.spawns.iter().map(|s| &s.name)),
        )
    })?;

    let mut app = make_app();
    app.finish().map_err(|d| vec![d])?;
    app.world
        .insert_reflected_resource("SimRng", SimRng::new(args.seed));
    waypoint::world::apply_scene(&mut app.world, scene);
    let (pos, heading) = waypoint::world::spawn_transform(spawn);
    let player = waypoint::vehicle::spawn_vehicle(&mut app.world, &vehicle.value, pos, heading);
    app.world.insert(player, Name(vehicle.value.name.clone()));
    app.world.insert(player, Player);
    *app.world.get_mut::<VehicleInput>(player).unwrap() = VehicleInput {
        throttle: args.throttle.clamp(0.0, 1.0),
        brake: args.brake.clamp(0.0, 1.0),
        steer: args.steer.clamp(-1.0, 1.0),
        reverse: args.reverse,
    };
    if let Some(tname) = &args.trailer {
        let tdef = &project
            .trailers
            .get(tname)
            .ok_or_else(|| {
                cli_error(
                    format!("unknown trailer `{tname}`"),
                    known(project.trailers.keys()),
                )
            })?
            .value;
        let t =
            waypoint::vehicle::spawn_trailer(&mut app.world, tdef, player).ok_or_else(|| {
                cli_error(
                    format!("vehicle `{}` has no fifth wheel", vehicle.value.name),
                    "pick a tractor unit with --vehicle",
                )
            })?;
        app.world.insert(t, Name(tdef.name.clone()));
    }
    Ok(Drive {
        app,
        player,
        label: format!("`{}` in {scene_path}", vehicle.value.name),
    })
}

fn camera_mode(name: &str) -> Result<CameraMode, Vec<Diagnostic>> {
    CameraMode::parse(name).ok_or_else(|| {
        cli_error(
            format!("unknown camera `{name}`"),
            "cameras: chase, top, side, showcase",
        )
    })
}

pub fn run(ctx: &Ctx, args: &RunArgs) -> CmdResult {
    let project = load_project(ctx)?;
    let Drive {
        mut app,
        player,
        label,
    } = setup_drive(ctx, &project, &args.drive)?;

    if !args.headless {
        let camera = camera_mode(&args.camera)?;
        let control = match args.control {
            Some(port) => {
                let (rx, addr) = wp_control::tcp::listen(port).map_err(|d| vec![d])?;
                eprintln!(
                    "control protocol listening on {addr} (JSON-RPC 2.0, one request per line)"
                );
                Some(rx)
            }
            None => None,
        };
        let session = wp_control::Session::new(app, Some(player), vehicle_map(&project));
        let summary = wp_platform::run_windowed(
            session,
            wp_platform::WindowOptions {
                title: format!("Waypoint · {}", project.game.value.name),
                width: 1280,
                height: 720,
                camera,
                exit_after_s: args.exit_after,
                control,
            },
        )
        .map_err(|d| vec![d])?;
        if ctx.json {
            print_json(&json!({ "ok": true, "window": summary }));
        } else {
            println!(
                "closed after {:.1} s: {} frames, {} ticks ({:.1} simulated s)",
                summary.real_seconds, summary.frames, summary.ticks, summary.sim_seconds
            );
        }
        return Ok(0);
    }

    let time = app.time();
    let ticks = args.ticks.unwrap_or_else(|| time.ticks_for(args.seconds));
    let mut trace = Vec::new();
    for tick in 1..=ticks {
        app.tick();
        if args.trace && tick % time.tick_hz as u64 == 0 {
            let tel = waypoint::vehicle::telemetry(&app.world, player).unwrap();
            if !ctx.json {
                println!(
                    "t={:>6.1}s  {:>6.1} km/h  gear {}  {:>5.0} rpm  lat {:+.2} g  pos ({:.1}, {:.1})",
                    app.time().seconds(),
                    tel.speed_kmh,
                    tel.gear,
                    tel.engine_rpm,
                    tel.lateral_g,
                    tel.position[0],
                    tel.position[1]
                );
            }
            trace.push(json!({ "t": app.time().seconds(), "telemetry": tel }));
        }
    }
    let tel = waypoint::vehicle::telemetry(&app.world, player).unwrap();
    let hash = app.world.state_hash();
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "ticks": ticks,
            "seconds": app.time().seconds(),
            "physics_hz": time.physics_hz(),
            "player": player.to_string(),
            "telemetry": tel,
            "state_hash": hash,
            "trace": trace,
            "world": app.world.to_json(),
        }));
    } else {
        println!(
            "ran {ticks} ticks ({:.1} s) of {label}: {:.1} km/h, gear {}, at ({:.1}, {:.1}), heading {:.1}°, state {hash}",
            app.time().seconds(),
            tel.speed_kmh,
            tel.gear,
            tel.position[0],
            tel.position[1],
            tel.heading_deg
        );
    }
    Ok(0)
}

pub(crate) fn vehicle_map(project: &Project) -> std::collections::BTreeMap<String, VehicleDef> {
    project
        .vehicles
        .iter()
        .map(|(k, v)| (k.clone(), v.value.clone()))
        .collect()
}

/// A control session for `drive`, used by `wp serve` and the MCP server.
pub(crate) fn make_session(
    ctx: &Ctx,
    drive: &DriveArgs,
) -> Result<wp_control::Session, Vec<Diagnostic>> {
    let project = load_project(ctx)?;
    let Drive { app, player, .. } = setup_drive(ctx, &project, drive)?;
    Ok(wp_control::Session::new(
        app,
        Some(player),
        vehicle_map(&project),
    ))
}

/// `wp serve`: JSON-RPC 2.0 on stdin/stdout, one request per line. Nothing else
/// is written to stdout, so any JSON-RPC client can talk to it.
pub fn serve(ctx: &Ctx, args: &ServeArgs) -> CmdResult {
    use std::io::{BufRead, Write};
    let mut session = make_session(ctx, &args.drive)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(resp) = session.handle_line(&line)
            && writeln!(stdout, "{resp}")
                .and_then(|_| stdout.flush())
                .is_err()
        {
            break;
        }
    }
    Ok(0)
}

fn parse_seconds(s: &str) -> Result<f32, Vec<Diagnostic>> {
    s.trim()
        .trim_end_matches('s')
        .parse::<f32>()
        .ok()
        .filter(|t| *t >= 0.0 && t.is_finite())
        .ok_or_else(|| {
            cli_error(
                format!("bad time `{s}`"),
                "use seconds, e.g. --at 12.5 or --at 12.5s",
            )
        })
}

pub fn shot(ctx: &Ctx, args: &ShotArgs) -> CmdResult {
    let at = parse_seconds(&args.at)?;
    let camera = camera_mode(&args.camera)?;
    if !(16..=8192).contains(&args.width) || !(16..=8192).contains(&args.height) {
        return Err(cli_error(
            "image size out of range",
            "width and height must be 16 to 8192 pixels",
        ));
    }
    let project = load_project(ctx)?;
    let Drive {
        mut app,
        player,
        label,
    } = setup_drive(ctx, &project, &args.drive)?;
    let ticks = app.time().ticks_for(at);
    app.run_ticks(ticks);

    let gpu = wp_rhi::Gpu::headless().map_err(|d| vec![d])?;
    let frame = wp_render::extract(&app.world, camera);
    let rgba =
        wp_render::render_rgba(&gpu, &frame, args.width, args.height).map_err(|d| vec![d])?;
    wp_rhi::write_png(&args.out, args.width, args.height, &rgba).map_err(|d| vec![d])?;
    let tel = waypoint::vehicle::telemetry(&app.world, player);
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "path": args.out,
            "width": args.width,
            "height": args.height,
            "at_s": app.time().seconds(),
            "ticks": ticks,
            "camera": args.camera,
            "gpu": gpu.info(),
            "telemetry": tel,
            "state_hash": app.world.state_hash(),
        }));
    } else {
        println!(
            "wrote {} ({}×{}, {} camera) of {label} at {:.2} s on {}",
            args.out.display(),
            args.width,
            args.height,
            args.camera,
            app.time().seconds(),
            gpu.info().name
        );
    }
    Ok(0)
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

// ---- wp test --------------------------------------------------------------

pub fn test(ctx: &Ctx, filter: Option<&str>) -> CmdResult {
    let project = load_project(ctx)?;
    let selected: Vec<_> = project
        .scenarios
        .iter()
        .filter(|(name, _)| filter.is_none_or(|f| name.contains(f)))
        .collect();
    if selected.is_empty() {
        return Err(vec![
            Diagnostic::error(
                "WP-C003",
                format!("no scenario matches `{}`", filter.unwrap_or("")),
            )
            .help(known(project.scenarios.keys())),
        ]);
    }
    let make_app = || waypoint::make_app(project.disabled_plugins());
    let mut reports: Vec<(String, ScenarioReport)> = Vec::new();
    for (name, sc) in selected {
        let _ = sc;
        let input = project.scenario_input(name).ok_or_else(|| {
            cli_error(format!("scenario `{name}` is incomplete"), "run `wp check`")
        })?;
        let report = run_scenario(&input, &make_app).map_err(|d| vec![d])?;
        if !ctx.json {
            print_report(&report, &sc.path);
        }
        reports.push((sc.path.clone(), report));
    }
    let failed = reports.iter().filter(|(_, r)| !r.passed).count();
    if ctx.json {
        let list: Vec<_> = reports
            .iter()
            .map(|(path, r)| json!({ "path": path, "report": r }))
            .collect();
        print_json(&json!({
            "passed": failed == 0,
            "total": reports.len(),
            "failed": failed,
            "reports": list,
        }));
    } else {
        println!(
            "\ntest result: {}. {} passed; {} failed",
            if failed == 0 { "ok" } else { "FAILED" },
            reports.len() - failed,
            failed
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

// ---- wp systems / plugins ---------------------------------------------------

fn finished_app(ctx: &Ctx) -> Result<App, Vec<Diagnostic>> {
    let disabled = match Project::load(&ctx.project) {
        (Some(p), _) => p.disabled_plugins().to_vec(),
        _ => Vec::new(),
    };
    let mut app = waypoint::make_app(&disabled);
    app.finish().map_err(|d| vec![d])?;
    Ok(app)
}

pub fn systems(ctx: &Ctx) -> CmdResult {
    let mut app = finished_app(ctx)?;
    let systems = app.systems();
    if ctx.json {
        print_json(&json!({ "physics_substeps": app.time().substeps, "systems": systems }));
    } else {
        let t = app.time();
        println!(
            "gameplay {} Hz; Physics stage runs {}× per tick ({} Hz)",
            t.tick_hz,
            t.substeps,
            t.physics_hz()
        );
        for s in systems {
            println!(
                "{:>2}  {:<14} {:<26} [{}]",
                s.order,
                format!("{:?}", s.stage),
                s.name,
                s.plugin
            );
            if !s.reads.is_empty() {
                println!("      reads  {}", s.reads.join(", "));
            }
            if !s.writes.is_empty() {
                println!("      writes {}", s.writes.join(", "));
            }
        }
    }
    Ok(0)
}

pub fn plugins(ctx: &Ctx) -> CmdResult {
    let app = finished_app(ctx)?;
    if ctx.json {
        print_json(&json!({ "plugins": app.plugins() }));
    } else {
        for p in app.plugins() {
            let deps = if p.dependencies.is_empty() {
                String::new()
            } else {
                format!("  needs {}", p.dependencies.join(", "))
            };
            println!("{:>2}  {} {}{}", p.order, p.name, p.version, deps);
        }
    }
    Ok(0)
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
