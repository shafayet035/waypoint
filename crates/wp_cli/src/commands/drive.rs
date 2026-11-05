//! Driving a session: `wp run` (window or headless), `wp shot`, `wp serve`.

use super::*;
use wp_audio as waypoint_audio;

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
    /// Play a mission (missions/<name>.mission.ron): its scene, vehicle and spawn.
    #[arg(long)]
    pub(crate) mission: Option<String>,
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
    /// Starting camera in a window: chase, cab, top, side, showcase.
    #[arg(long, default_value = "chase")]
    camera: String,
    /// Quality tier for the window: low, mid or flagship (default: game.toml's).
    #[arg(long)]
    quality: Option<String>,
    /// Close the window after this many real seconds (for smoke tests).
    #[arg(long)]
    exit_after: Option<f32>,
    /// Keep the tier's full quality even when frames run late or the device is hot.
    #[arg(long)]
    no_governor: bool,
    /// Pretend the device is this hot (nominal, fair, serious, critical) to test the governor.
    #[arg(long)]
    thermal: Option<String>,
    /// Accept control-protocol requests on 127.0.0.1:<port> while the window runs (0 = any free port).
    #[arg(long)]
    control: Option<u16>,
    /// Record the drive to this .replay.ron (inputs, resets, world hashes); check it with `wp replay`.
    #[arg(long)]
    record: Option<std::path::PathBuf>,
    /// Ticks between world hashes in a recording.
    #[arg(long, default_value_t = 60)]
    hash_every: u64,
    /// No engine sound in the window.
    #[arg(long)]
    mute: bool,
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
    pub(crate) fn spec(&self) -> waypoint::drive::DriveSpec {
        waypoint::drive::DriveSpec {
            scenario: self.scenario.clone(),
            scene: self.scene.clone(),
            spawn: self.spawn.clone(),
            vehicle: self.vehicle.clone(),
            trailer: self.trailer.clone(),
            mission: self.mission.clone(),
            quality: None,
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
            "cameras: chase, cab, top, side, showcase",
        )
    })
}

pub fn run(ctx: &Ctx, args: &RunArgs) -> CmdResult {
    let project = load_project(ctx)?;
    let tier = match args.quality.as_deref() {
        None => project.game.value.quality.tier,
        Some("auto") => {
            // The tier the player would use on this machine (measured once).
            let path = wp_platform::settings::settings_path(None, &project.game.value.name);
            let (tier, how) = wp_platform::settings::device_tier(
                path.as_deref(),
                project.game.value.quality.tier,
            );
            eprintln!("{}", how.describe(tier));
            tier
        }
        Some("low") => waypoint::world::QualityTier::Low,
        Some("mid") => waypoint::world::QualityTier::Mid,
        Some("flagship") => waypoint::world::QualityTier::Flagship,
        Some(other) => {
            return Err(cli_error(
                format!("unknown quality tier `{other}`"),
                "tiers: low, mid, flagship, auto",
            ));
        }
    };
    let mut spec = args.drive.spec();
    if !args.headless {
        spec.quality = Some(tier);
    }
    let Drive {
        mut app,
        player,
        label,
    } = spec.build(&project).map_err(|d| vec![d])?;

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
        let mut session = wp_control::Session::new(app, Some(player), vehicle_map(&project));
        session.setup = args
            .drive
            .replay_setup(Some(super::replay::tier_name(tier)));
        if args.record.is_some() {
            session.start_recording(args.hash_every);
        }
        let thermal = match args.thermal.as_deref() {
            None => None,
            Some(t) => Some(wp_render::governor::Thermal::parse(t).ok_or_else(|| {
                cli_error(
                    format!("unknown thermal state `{t}`"),
                    "states: nominal, fair, serious, critical",
                )
            })?),
        };
        let summary = wp_platform::run_windowed(
            session,
            wp_platform::WindowOptions {
                title: format!("Waypoint · {}", project.game.value.name),
                width: 1280,
                height: 720,
                camera,
                exit_after_s: args.exit_after,
                control,
                quality: Some(tier),
                audio: !args.mute,
                governor: !args.no_governor,
                thermal,
            },
        )
        .map_err(|d| vec![d])?;
        if let (Some(path), Some(replay)) = (&args.record, &summary.replay) {
            super::replay::write_replay(path, replay)?;
            if !ctx.json {
                println!("recorded {} ticks to {}", replay.ticks, path.display());
            }
        }
        if ctx.json {
            print_json(&json!({ "ok": true, "window": summary }));
        } else {
            println!(
                "closed after {:.1} s: {} frames, {} ticks ({:.1} simulated s)",
                summary.real_seconds, summary.frames, summary.ticks, summary.sim_seconds
            );
            if let Some(g) = &summary.governor {
                println!(
                    "render step {} of 5 ({} changes, {:?} heat, {:.1} ms frames, aiming for {} fps)",
                    g.step, g.changes, g.thermal, g.frame_ms, g.target_fps
                );
            }
        }
        return Ok(0);
    }

    let time = app.time();
    let ticks = args.ticks.unwrap_or_else(|| time.ticks_for(args.seconds));
    let mut trace = Vec::new();
    let mut recorder = args.record.as_ref().map(|_| {
        waypoint::vehicle::replay::Recorder::new(args.drive.replay_setup(None), args.hash_every)
    });
    for tick in 1..=ticks {
        if let Some(r) = &mut recorder {
            let input = app
                .world
                .get::<VehicleInput>(player)
                .map(|i| *i)
                .unwrap_or_default();
            r.before_tick(input);
        }
        app.tick();
        if let Some(r) = &mut recorder {
            r.after_tick(&app.world);
        }
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
    if let (Some(path), Some(r)) = (&args.record, recorder) {
        super::replay::write_replay(path, &r.finish(&app.world))?;
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
    // Pushed event notifications are written before the response of the
    // request (usually sim.step) during which they happened.
    let (push_tx, push_rx) = std::sync::mpsc::channel::<String>();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let resp = session.handle_line_with(&line, &push_tx);
        let mut out: Vec<String> = push_rx.try_iter().collect();
        out.extend(resp);
        if out
            .iter()
            .try_for_each(|l| writeln!(stdout, "{l}"))
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
    let (rgba, stats) = wp_render::render_rgba_with_stats(&gpu, &frame, args.width, args.height)
        .map_err(|d| vec![d])?;
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
            "render": stats,
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

// ---- wp audio ----------------------------------------------------------------

#[derive(clap::Args)]
pub struct AudioArgs {
    /// Simulated seconds to record (a scenario's own length if omitted).
    #[arg(long)]
    seconds: Option<f32>,
    /// Output WAV path (mono, 16-bit, 44.1 kHz).
    #[arg(long, default_value = "engine.wav")]
    out: std::path::PathBuf,
    #[command(flatten)]
    drive: DriveArgs,
}

/// `wp audio`: render the player vehicle's sound for a drive or scenario to WAV.
pub fn audio(ctx: &Ctx, args: &AudioArgs) -> CmdResult {
    const RATE: u32 = 44_100;
    let project = load_project(ctx)?;
    let Drive {
        mut app,
        player,
        label,
    } = setup_drive(&project, &args.drive)?;
    let seconds = args
        .seconds
        .or_else(|| {
            args.drive
                .scenario
                .as_ref()
                .and_then(|s| project.scenarios.get(s))
                .map(|s| s.value.duration_s)
        })
        .unwrap_or(10.0);
    let ticks = app.time().ticks_for(seconds);
    let per_tick = RATE as usize / app.time().tick_hz as usize;
    let mut mixer = waypoint_audio::Mixer::new(RATE);
    let mut samples = Vec::with_capacity(ticks as usize * per_tick);
    let (mut left, mut right) = (vec![0.0f32; per_tick], vec![0.0f32; per_tick]);
    let mut peak_rpm = 0.0f32;
    for _ in 0..ticks {
        app.tick();
        // The player's vehicle and the traffic around it, mixed to mono.
        let sources = wp_platform::audio::sound_sources(&app.world, player);
        if let Some(own) = sources.first() {
            peak_rpm = peak_rpm.max(own.input.rpm);
        }
        mixer.render(&sources, &mut left, &mut right);
        samples.extend(left.iter().zip(&right).map(|(l, r)| (l + r) * 0.5));
    }
    std::fs::write(&args.out, waypoint_audio::wav_bytes(&samples, RATE)).map_err(|e| {
        vec![Diagnostic::error(
            "WP-D001",
            format!("cannot write {}: {e}", args.out.display()),
        )]
    })?;
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "path": args.out,
            "seconds": seconds,
            "sample_rate": RATE,
            "peak_rpm": peak_rpm,
            "rms": rms,
        }));
    } else {
        println!(
            "wrote {} ({seconds:.1} s of {label}; peak {peak_rpm:.0} rpm, level {rms:.3})",
            args.out.display()
        );
    }
    Ok(0)
}
