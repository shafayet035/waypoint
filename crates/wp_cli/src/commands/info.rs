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
