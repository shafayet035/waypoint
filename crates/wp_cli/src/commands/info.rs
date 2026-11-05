//! `wp systems` and `wp plugins`: what the engine has loaded.

use super::*;

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

// ---- wp bench ----------------------------------------------------------------

/// Run the renderer benchmark headless and report the tier this device gets
/// (what the player picks on first launch when game.toml has `auto = true`).
pub fn bench(ctx: &Ctx) -> CmdResult {
    let gpu = wp_rhi::Gpu::headless().map_err(|d| vec![d])?;
    let report = wp_render::bench::run(&gpu).map_err(|d| vec![d])?;
    let memory_gb = wp_rhi::system_memory_gb();
    let tier =
        waypoint::world::QualityTier::for_device(report.frame_ms, memory_gb, report.software());
    if ctx.json {
        print_json(&json!({ "bench": report, "memory_gb": memory_gb, "tier": tier.name() }));
    } else {
        println!(
            "{} frames of the benchmark town ({}×{}, Mid settings, rain at night) on {} ({}, {})",
            report.frames,
            report.width,
            report.height,
            report.gpu.name,
            report.gpu.backend,
            report.gpu.device_type
        );
        println!(
            "  frame time: median {:.2} ms, 90th percentile {:.2} ms ({} draw calls, {} triangles)",
            report.frame_ms, report.p90_ms, report.draw_calls, report.triangles
        );
        println!(
            "  memory: {}",
            memory_gb.map_or("unknown".to_string(), |m| format!("{m:.1} GB"))
        );
        println!(
            "tier: {} (Flagship needs ≤ 6 ms and 7 GB, Mid ≤ 12 ms and 5 GB)",
            tier.name()
        );
    }
    Ok(0)
}
