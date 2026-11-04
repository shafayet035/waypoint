//! `wp mod`: list mods and start new ones.

use super::*;

/// `wp mod list`: every mod, whether it is active, and what it changes.
pub fn mod_list(ctx: &Ctx) -> CmdResult {
    let (project, diags) = Project::load(&ctx.project);
    let Some(project) = project else {
        return Err(diags.0);
    };
    let list: Vec<_> = project
        .mods
        .iter()
        .map(|m| {
            let code = &m.manifest.value.code;
            let (replaces, adds): (Vec<&String>, Vec<&String>) = m
                .files
                .iter()
                .filter(|f| !code.contains(f))
                .partition(|f| project.vfs.in_base(f));
            json!({
                "id": m.id,
                "name": m.manifest.value.name,
                "version": m.manifest.value.version,
                "author": m.manifest.value.author,
                "priority": m.manifest.value.priority,
                "active": m.active,
                "adds": adds,
                "replaces": replaces,
                "code": code,
                "capabilities": m.manifest.value.capabilities,
            })
        })
        .collect();
    if ctx.json {
        print_json(&json!({ "mods": list }));
        return Ok(0);
    }
    if list.is_empty() {
        println!("no mods; start one with `wp mod new <name>`");
    }
    for m in &list {
        println!(
            "{} {} {} ({}){}",
            if m["active"] == true { "●" } else { "○" },
            m["id"].as_str().unwrap_or(""),
            m["version"].as_str().unwrap_or(""),
            m["name"].as_str().unwrap_or(""),
            if m["active"] == true {
                ""
            } else {
                " — disabled"
            }
        );
        for (what, key) in [("adds", "adds"), ("replaces", "replaces"), ("runs", "code")] {
            for f in m[key].as_array().into_iter().flatten() {
                println!("    {what} {}", f.as_str().unwrap_or(""));
            }
        }
    }
    Ok(0)
}

/// `wp mod new <name>`: a mod folder with a manifest and a README.
pub fn mod_new(ctx: &Ctx, name: &str) -> CmdResult {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !ok {
        return Err(cli_error(
            format!("bad mod name `{name}`"),
            "use lowercase letters, digits, - and _, e.g. night_bus",
        ));
    }
    load_project(ctx)?;
    let dir = ctx.project.join("mods").join(name);
    if dir.exists() {
        return Err(vec![
            Diagnostic::error("WP-C004", format!("{} already exists", dir.display()))
                .help("choose a new name"),
        ]);
    }
    std::fs::create_dir_all(&dir).map_err(|e| cli_error(e.to_string(), "check permissions"))?;
    let manifest = format!(
        "# A Waypoint mod. Files here mirror the game's layout: a file at the same path\n\
         # replaces the game's (vehicles/hatchback.vehicle.ron), a new path adds one.\n\
         name = \"{name}\"\n\
         version = \"0.1.0\"\n\
         author = \"\"\n\
         description = \"\"\n\
         # Load order: lower first; later mods override earlier ones.\n\
         priority = 0\n"
    );
    let readme = format!(
        "# {name}\n\n\
         Add or replace game files by putting them at the same paths here:\n\n\
         - `vehicles/<name>.vehicle.ron`, `scenes/`, `roads/`, `missions/` (+ `.lua`), `tests/`\n\
         - `assets/` models, each with its `.asset.ron` licence file\n\n\
         Check with `wp check` (errors point at files in this folder), see what the mod\n\
         changes with `wp mod list`, and switch it off in game.toml:\n\n\
         ```toml\n[mods]\ndisabled = [\"{name}\"]\n```\n"
    );
    for (file, text) in [("mod.toml", manifest), ("README.md", readme)] {
        std::fs::write(dir.join(file), text)
            .map_err(|e| cli_error(e.to_string(), "check permissions"))?;
    }
    if ctx.json {
        print_json(&json!({ "ok": true, "path": dir }));
    } else {
        println!(
            "created {}; add files to it, then `wp check`",
            dir.display()
        );
    }
    Ok(0)
}
