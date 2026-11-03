//! Driving a session: `wp run` (window or headless), `wp shot`, `wp serve`.

use super::*;

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

impl DriveArgs {
    fn spec(&self) -> waypoint::drive::DriveSpec {
        waypoint::drive::DriveSpec {
            scenario: self.scenario.clone(),
            scene: self.scene.clone(),
            spawn: self.spawn.clone(),
            vehicle: self.vehicle.clone(),
            trailer: self.trailer.clone(),
            input: VehicleInput {
                throttle: self.throttle,
                brake: self.brake,
                steer: self.steer,
                reverse: self.reverse,
            },
            seed: self.seed,
        }
    }
}

fn setup_drive(project: &Project, args: &DriveArgs) -> Result<Drive, Vec<Diagnostic>> {
    args.spec().build(project).map_err(|d| vec![d])
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
    } = setup_drive(&project, &args.drive)?;

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
    let Drive { app, player, .. } = setup_drive(&project, drive)?;
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
    } = setup_drive(&project, &args.drive)?;
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
