//! Screenshot tests in `wp test`: render each `tests/<name>.shot.ron` and
//! compare it with its approved picture; `wp test --approve` saves new ones.

use super::*;
use waypoint::test::ShotDef;

/// Where failed shots leave the new picture and the diff, under the project.
pub const SHOT_OUT_DIR: &str = "target/shots";

/// One screenshot test's outcome.
#[derive(serde::Serialize)]
pub struct ShotResult {
    pub name: String,
    pub path: String,
    pub passed: bool,
    /// Why it did not run (no GPU), if so; skipped tests do not fail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<wp_render::compare::ImageDiff>,
    /// The approved picture, and (when it failed) the new one and the diff.
    pub approved: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_image: Option<String>,
    pub wall_ms: u128,
}

/// Run shot `def` and render its frame: RGBA8 at its size.
fn render(project: &Project, gpu: &wp_rhi::Gpu, def: &ShotDef) -> Result<Vec<u8>, Vec<Diagnostic>> {
    let spec = waypoint::drive::DriveSpec {
        scenario: def.scenario.clone(),
        scene: def.scene.clone(),
        spawn: def.spawn.clone(),
        vehicle: def.vehicle.clone(),
        trailer: None,
        mission: None,
        quality: def.quality,
        input: VehicleInput {
            throttle: def.throttle,
            brake: 0.0,
            steer: def.steer,
            reverse: false,
        },
        seed: def.seed,
    };
    let Drive { mut app, .. } = spec.build(project).map_err(|d| vec![d])?;
    let ticks = app.time().ticks_for(def.at_s);
    app.run_ticks(ticks);
    let mode =
        wp_render::CameraMode::parse(def.camera.name()).expect("every ShotCamera has a mode");
    let frame = wp_render::extract(&app.world, mode);
    wp_render::render_rgba_at(gpu, &frame, def.width, def.height, def.quality)
        .map(|(rgba, _)| rgba)
        .map_err(|d| vec![d])
}

/// Run the selected screenshot tests. `gpu` is `Err(why)` without a GPU:
/// every test is then skipped.
pub fn run_shots(
    project: &Project,
    names: &[&String],
    gpu: &Result<wp_rhi::Gpu, Diagnostic>,
) -> Result<Vec<ShotResult>, Vec<Diagnostic>> {
    let mut out = Vec::new();
    for name in names {
        let f = &project.shots[*name];
        let approved = project.shot_image(name).unwrap_or_default();
        let mut r = ShotResult {
            name: name.to_string(),
            path: f.path.clone(),
            passed: false,
            skipped: None,
            detail: String::new(),
            diff: None,
            approved: approved.clone(),
            actual: None,
            diff_image: None,
            wall_ms: 0,
        };
        let gpu = match gpu {
            Ok(g) => g,
            Err(d) => {
                r.passed = true;
                r.skipped = Some(format!("no GPU: {}", d.message));
                r.detail = "skipped (no GPU adapter)".into();
                out.push(r);
                continue;
            }
        };
        let started = std::time::Instant::now();
        let def = &f.value;
        let rgba = render(project, gpu, def)?;
        r.wall_ms = started.elapsed().as_millis();
        let save_actual =
            |r: &mut ShotResult, diff: Option<Vec<u8>>| -> Result<(), Vec<Diagnostic>> {
                let dir = project.root.join(SHOT_OUT_DIR);
                let actual = dir.join(format!("{name}.png"));
                wp_rhi::write_png(&actual, def.width, def.height, &rgba).map_err(|d| vec![d])?;
                r.actual = Some(format!("{SHOT_OUT_DIR}/{name}.png"));
                if let Some(img) = diff {
                    let path = dir.join(format!("{name}.diff.png"));
                    wp_rhi::write_png(&path, def.width, def.height, &img).map_err(|d| vec![d])?;
                    r.diff_image = Some(format!("{SHOT_OUT_DIR}/{name}.diff.png"));
                }
                Ok(())
            };
        let approved_path = project.root.join(&approved);
        if !approved_path.is_file() {
            save_actual(&mut r, None)?;
            r.detail = format!(
                "no approved picture yet; look at {SHOT_OUT_DIR}/{name}.png, then `wp test --approve {name}`"
            );
            out.push(r);
            continue;
        }
        let (w, h, golden) = wp_rhi::read_png(&approved_path).map_err(|d| vec![d])?;
        if (w, h) != (def.width, def.height) {
            save_actual(&mut r, None)?;
            r.detail = format!(
                "approved picture is {w}×{h}, the test renders {}×{}; approve it again",
                def.width, def.height
            );
            out.push(r);
            continue;
        }
        let t = def.tolerance;
        let diff = wp_render::compare::compare(&golden, &rgba, w, h, t.pixel);
        r.passed = diff.changed_pct <= t.changed_pct;
        r.diff = Some(diff);
        r.detail = format!(
            "{:.2} % of pixels changed (≤ {} %), mean difference {:.4}",
            diff.changed_pct, t.changed_pct, diff.mean
        );
        if !r.passed {
            let img = wp_render::compare::diff_image(&golden, &rgba, w, h, t.pixel);
            save_actual(&mut r, Some(img))?;
            if let Some([x0, y0, x1, y1]) = diff.changed_area {
                r.detail += &format!(" in x {x0}–{x1}, y {y0}–{y1}");
            }
        }
        out.push(r);
    }
    Ok(out)
}

/// `wp test --approve`: render the selected screenshot tests and save their
/// pictures as the approved ones.
pub fn approve(ctx: &Ctx, project: &Project, names: &[&String]) -> CmdResult {
    let gpu = wp_rhi::Gpu::headless().map_err(|d| vec![d])?;
    let mut written = Vec::new();
    for name in names {
        let def = &project.shots[*name].value;
        let rgba = render(project, &gpu, def)?;
        let rel = project.shot_image(name).unwrap_or_default();
        wp_rhi::write_png(&project.root.join(&rel), def.width, def.height, &rgba)
            .map_err(|d| vec![d])?;
        written.push(rel);
    }
    if ctx.json {
        print_json(&json!({ "ok": true, "approved": written, "gpu": gpu.info() }));
    } else {
        for w in &written {
            println!("approved {w}");
        }
        println!("rendered on {}", gpu.info().name);
    }
    Ok(0)
}

pub fn print_shot(r: &ShotResult) {
    let status = if r.skipped.is_some() {
        "skipped"
    } else if r.passed {
        "ok"
    } else {
        "FAILED"
    };
    println!("shot {} ... {status} ({} ms)", r.name, r.wall_ms);
    println!(
        "  {} {:<36} {}",
        if r.passed { "✓" } else { "✗" },
        "PictureMatches",
        r.detail
    );
    if !r.passed {
        println!("  --> {}", r.path);
        if let Some(a) = &r.actual {
            println!("  new picture: {a}");
        }
        if let Some(d) = &r.diff_image {
            println!("  changes (magenta): {d}");
        }
        println!(
            "  if the change is intended: `wp test --approve {}`",
            r.name
        );
    }
}
