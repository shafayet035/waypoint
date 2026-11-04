//! `wp replay`: record a run's inputs and world hashes, then check that it
//! still plays back identically (golden replays).

use super::*;
use waypoint::vehicle::replay::{Divergence, Playback, Recorder, Replay, ReplaySetup, StartState};

impl DriveArgs {
    /// The setup a recording of this drive should carry.
    pub(crate) fn replay_setup(&self, quality: Option<&str>) -> ReplaySetup {
        ReplaySetup {
            scenario: self.scenario.clone(),
            scene: self.scene.clone(),
            spawn: self.spawn.clone(),
            vehicle: self.vehicle.clone(),
            trailer: self.trailer.clone(),
            mission: self.mission.clone(),
            quality: quality.map(str::to_string),
            seed: self.seed,
        }
    }

    fn from_setup(s: &ReplaySetup) -> DriveArgs {
        DriveArgs {
            scenario: s.scenario.clone(),
            scene: s.scene.clone(),
            spawn: s.spawn.clone(),
            vehicle: s.vehicle.clone(),
            trailer: s.trailer.clone(),
            mission: s.mission.clone(),
            seed: s.seed,
            ..Default::default()
        }
    }
}

pub(crate) fn tier_name(t: waypoint::world::QualityTier) -> &'static str {
    match t {
        waypoint::world::QualityTier::Low => "low",
        waypoint::world::QualityTier::Mid => "mid",
        waypoint::world::QualityTier::Flagship => "flagship",
    }
}

pub(crate) fn parse_tier(name: &str) -> Result<waypoint::world::QualityTier, Vec<Diagnostic>> {
    match name {
        "low" => Ok(waypoint::world::QualityTier::Low),
        "mid" => Ok(waypoint::world::QualityTier::Mid),
        "flagship" => Ok(waypoint::world::QualityTier::Flagship),
        other => Err(cli_error(
            format!("unknown quality tier `{other}`"),
            "tiers: low, mid, flagship",
        )),
    }
}

/// Write a replay as RON, one checkpoint and input change per line.
pub(crate) fn write_replay(path: &Path, replay: &Replay) -> Result<(), Vec<Diagnostic>> {
    let text = waypoint::data::to_ron_depth(replay, 2).map_err(|d| vec![d])?;
    let header = "// Recorded by `wp replay record` / `wp run --record`; check it with `wp replay <file>`.\n";
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, format!("{header}{text}\n")).map_err(|e| {
        vec![
            Diagnostic::error("WP-D001", format!("cannot write {}: {e}", path.display()))
                .help("check the folder exists and is writable"),
        ]
    })
}

/// Play `replay` back headless; the first divergence, if any.
fn play(project: &Project, replay: &Replay) -> Result<Option<Divergence>, Vec<Diagnostic>> {
    let mut spec = DriveArgs::from_setup(&replay.setup).spec();
    spec.quality = replay
        .setup
        .quality
        .as_deref()
        .map(parse_tier)
        .transpose()?;
    let Drive {
        mut app, player, ..
    } = spec.build(project).map_err(|d| vec![d])?;
    let start = StartState::capture(&app.world, player).ok_or_else(|| {
        cli_error(
            "the replay's player is not a vehicle",
            "re-record the replay",
        )
    })?;
    let mut p = Playback::new(replay.clone());
    while !p.done() {
        p.before_tick(&mut app.world, player, &start);
        app.tick();
        if let Some(d) = p.after_tick(&app.world) {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

/// `wp replay <file>`: play a recording back and compare every checkpoint.
pub fn replay_verify(ctx: &Ctx, file: &Path) -> CmdResult {
    let project = load_project(ctx)?;
    let rel = file.to_string_lossy().to_string();
    let path = if file.is_absolute() || file.exists() {
        file.to_path_buf()
    } else {
        ctx.project.join(file)
    };
    let text = std::fs::read_to_string(&path).map_err(|e| {
        vec![Diagnostic::error("WP-D001", format!("cannot read replay: {e}")).in_file(rel.clone())]
    })?;
    let replay: Replay = waypoint::data::parse_ron(&rel, &text).map_err(|d| vec![d])?;
    let hz = 60.0;
    let divergence = play(&project, &replay)?;
    let diag = divergence.as_ref().map(|_| {
        Diagnostic::error("WP-Y002", format!("{rel} no longer plays back identically"))
            .help("something changed the simulation; `wp shot` at the divergence time shows the state, and re-recording accepts the change")
    });
    if ctx.json {
        print_json(&json!({
            "ok": divergence.is_none(),
            "ticks": replay.ticks,
            "checkpoints": replay.hashes.len() + 1,
            "divergence": divergence,
            "diagnostics": diag.iter().collect::<Vec<_>>(),
        }));
        return Ok(if divergence.is_some() { 1 } else { 0 });
    }
    match &divergence {
        None => println!(
            "{rel}: identical — {} checkpoints over {} ticks ({:.1} s)",
            replay.hashes.len() + 1,
            replay.ticks,
            replay.ticks as f64 / hz
        ),
        Some(d) => println!(
            "{rel}: DIVERGED by tick {} ({:.2} s); last match at tick {} ({:.2} s)\n  expected {}, got {}",
            d.tick,
            d.tick as f64 / hz,
            d.last_good_tick,
            d.last_good_tick as f64 / hz,
            d.expected,
            d.got
        ),
    }
    match diag {
        Some(d) => Err(vec![d]),
        None => Ok(0),
    }
}

/// `wp replay record --scenario <name>`: a golden replay next to the scenario.
pub fn replay_record(ctx: &Ctx, scenario: &str, out: Option<&Path>, hash_every: u64) -> CmdResult {
    let project = load_project(ctx)?;
    let sc = project.scenarios.get(scenario).ok_or_else(|| {
        vec![
            Diagnostic::error("WP-C003", format!("no scenario `{scenario}`"))
                .help(known(project.scenarios.keys())),
        ]
    })?;
    let Drive {
        mut app, player, ..
    } = waypoint::drive::DriveSpec::scenario(scenario)
        .build(&project)
        .map_err(|d| vec![d])?;
    let setup = ReplaySetup {
        scenario: Some(scenario.to_string()),
        seed: sc.value.seed,
        ..Default::default()
    };
    let mut rec = Recorder::new(setup, hash_every);
    let ticks = app.time().ticks_for(sc.value.duration_s);
    for _ in 0..ticks {
        let input = app
            .world
            .get::<VehicleInput>(player)
            .map(|i| *i)
            .unwrap_or_default();
        rec.before_tick(input);
        app.tick();
        rec.after_tick(&app.world);
    }
    let mut replay = rec.finish(&app.world);
    // The scenario's driver makes the inputs again on playback.
    replay.inputs.clear();
    let default_out = ctx
        .project
        .join(sc.path.replace(".scenario.ron", ".replay.ron"));
    let out = out.map(Path::to_path_buf).unwrap_or(default_out);
    write_replay(&out, &replay)?;
    if ctx.json {
        print_json(
            &json!({ "ok": true, "path": out, "ticks": replay.ticks, "checkpoints": replay.hashes.len() + 1, "final_hash": replay.final_hash }),
        );
    } else {
        println!(
            "wrote {} — {} checkpoints over {} ticks; `wp test {scenario}` now checks it",
            out.display(),
            replay.hashes.len() + 1,
            replay.ticks
        );
    }
    Ok(0)
}
