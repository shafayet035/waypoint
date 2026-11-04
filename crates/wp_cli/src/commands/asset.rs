//! `wp asset` and `wp credits`: models, their licences and the credits screen.

use super::*;
use waypoint::asset::{Model, Role};

fn model_json(path: &str, m: &Model, project: Option<&Project>) -> serde_json::Value {
    let meta = project.and_then(|p| p.assets.get(path)).map(|a| &a.value);
    let used_by: Vec<&String> = project
        .map(|p| {
            p.vehicles
                .values()
                .filter(|v| v.value.model.as_deref() == Some(path))
                .map(|v| &v.value.name)
                .collect()
        })
        .unwrap_or_default();
    json!({
        "path": path,
        "triangles": m.triangles,
        "lod_triangles": m.lods.iter().map(|l| l.iter().map(|p| p.indices.len() / 3).sum::<usize>()).collect::<Vec<_>>(),
        "size_m": m.size().to_array(),
        "min": m.min.to_array(),
        "max": m.max.to_array(),
        "textures": m.textures,
        "parts": m.parts.iter().map(|p| json!({
            "material": p.material.name,
            "role": format!("{:?}", p.material.role),
            "triangles": p.indices.len() / 3,
            "base_color": p.material.base_color,
            "roughness": p.material.roughness,
            "metallic": p.material.metallic,
            "emissive": p.material.emissive,
        })).collect::<Vec<_>>(),
        "licence": meta.map(|m| m.licence.label()),
        "author": meta.map(|m| m.author.clone()),
        "source": meta.map(|m| m.source.clone()),
        "used_by": used_by,
    })
}

/// `wp asset info <file>`: what the engine sees in a model.
pub fn asset_info(ctx: &Ctx, file: &str) -> CmdResult {
    let (project, _) = Project::load(&ctx.project);
    let rel = file.trim_start_matches("./").to_string();
    let model = match project.as_ref().and_then(|p| p.models.find(&rel)) {
        Some((_, m)) => (**m).clone(),
        None => {
            // A file outside the project (or not loaded yet): read it directly.
            let path = std::path::Path::new(file);
            let (root, name) = if path.is_absolute() || path.exists() {
                (std::path::Path::new(""), file)
            } else {
                (ctx.project.as_path(), file)
            };
            waypoint::asset::load_model(root, name).map_err(|d| vec![d])?
        }
    };
    if ctx.json {
        print_json(&model_json(&rel, &model, project.as_ref()));
        return Ok(0);
    }
    let s = model.size();
    println!(
        "{rel}: {} triangles, {:.2} × {:.2} × {:.2} m (length × width × height), lowest point {:.2} m",
        model.triangles, s.x, s.y, s.z, model.min.z
    );
    let lods: Vec<String> = model
        .lods
        .iter()
        .map(|l| {
            l.iter()
                .map(|p| p.indices.len() / 3)
                .sum::<usize>()
                .to_string()
        })
        .collect();
    println!("  distance levels: {} triangles", lods.join(", "));
    if model.textures > 0 {
        println!(
            "  {} textures (base colour textures are drawn; others are not used yet)",
            model.textures
        );
    }
    for p in &model.parts {
        let role = match p.material.role {
            Role::Paint => "paint (vehicle colour)",
            Role::Glass => "glass",
            Role::BrakeLight => "brake light",
            Role::HeadLight => "headlight",
            Role::Plain => "plain",
        };
        println!(
            "  {:<14} {:>6} triangles  {role}",
            if p.material.name.is_empty() {
                "(default)"
            } else {
                &p.material.name
            },
            p.indices.len() / 3
        );
    }
    if let Some(m) = project.as_ref().and_then(|p| p.assets.get(&rel)) {
        println!(
            "  licence {} — {} by {}",
            m.value.licence.label(),
            m.value.source,
            m.value.author
        );
    }
    Ok(0)
}

/// `wp asset list`: every model with its size, budget use and licence.
pub fn asset_list(ctx: &Ctx) -> CmdResult {
    let (project, diags) = Project::load(&ctx.project);
    let Some(project) = project else {
        return Err(diags.0);
    };
    let list: Vec<_> = project
        .models
        .iter()
        .map(|(p, m)| model_json(p, m, Some(&project)))
        .collect();
    if ctx.json {
        print_json(&json!({ "assets": list }));
        return Ok(0);
    }
    if list.is_empty() {
        println!(
            "no models; put .glb files under assets/ with a .asset.ron licence file next to each"
        );
    }
    for a in &list {
        println!(
            "{}  {} triangles  {}  used by: {}",
            a["path"].as_str().unwrap_or(""),
            a["triangles"],
            a["licence"].as_str().unwrap_or("NO LICENCE FILE"),
            a["used_by"]
                .as_array()
                .filter(|v| !v.is_empty())
                .map(|v| v
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_else(|| "(nothing)".into())
        );
    }
    Ok(0)
}

/// `wp credits`: the lines for the game's credits screen.
pub fn credits(ctx: &Ctx) -> CmdResult {
    let project = load_project(ctx)?;
    let mut lines: Vec<String> = project.credits().into_iter().map(|(_, l)| l).collect();
    if project
        .roads
        .values()
        .any(|r| r.source.contains("OpenStreetMap contributors"))
    {
        lines.push(
            "Map data © OpenStreetMap contributors, ODbL (openstreetmap.org/copyright)".into(),
        );
    }
    if ctx.json {
        print_json(&json!({ "credits": lines }));
    } else {
        for l in &lines {
            println!("{l}");
        }
    }
    Ok(0)
}

/// `wp asset build`: check every model and report its levels of detail
/// against each quality tier's triangle budget. Exit 1 on any error.
pub fn asset_build(
    ctx: &Ctx,
    tier: Option<&str>,
    out: Option<&Path>,
    with_tests: bool,
    gpu_compress: bool,
) -> CmdResult {
    let (project, diags) = Project::load(&ctx.project);
    let Some(project) = project else {
        return Err(diags.0);
    };
    let asset_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.code.starts_with("WP-F"))
        .cloned()
        .collect();
    let tiers = ["low", "mid", "flagship"];
    let rows: Vec<_> = project
        .models
        .iter()
        .map(|(path, m)| {
            let levels: Vec<usize> = std::iter::once(m.triangles as usize)
                .chain(m.lods.iter().map(|l| l.iter().map(|p| p.indices.len() / 3).sum()))
                .collect();
            let fits: Vec<bool> = waypoint::asset::PLAYER_TRIANGLES
                .iter()
                .map(|b| m.triangles <= *b)
                .collect();
            json!({ "path": path, "levels": levels, "fits_player_budget": tiers.iter().zip(&fits).map(|(t, f)| (t.to_string(), json!(f))).collect::<serde_json::Map<_, _>>() })
        })
        .collect();
    let errors = asset_diags
        .iter()
        .any(|d| d.severity == waypoint::core::Severity::Error);
    if ctx.json && (out.is_none() || errors) {
        print_json(&json!({ "ok": !errors, "assets": rows, "diagnostics": asset_diags }));
    } else if !ctx.json {
        for d in &asset_diags {
            print!("{}", d.render());
        }
        println!(
            "{:<40} {:>8} {:>8} {:>8}   budget (low/mid/flagship)",
            "model", "full", "lod 1", "lod 2"
        );
        for r in &rows {
            let l: Vec<u64> = r["levels"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_u64()).collect())
                .unwrap_or_default();
            let at = |i: usize| l.get(i).copied().unwrap_or(0);
            let fit = |t: &str| {
                if r["fits_player_budget"][t] == true {
                    "ok"
                } else {
                    "OVER"
                }
            };
            println!(
                "{:<40} {:>8} {:>8} {:>8}   {}/{}/{}",
                r["path"].as_str().unwrap_or(""),
                at(0),
                at(1),
                at(2),
                fit("low"),
                fit("mid"),
                fit("flagship")
            );
        }
        println!("levels are built when the game loads");
    }
    if errors {
        return Ok(1);
    }
    match out {
        Some(dir) => ship(ctx, &project, tier, dir, with_tests, gpu_compress, rows),
        None => Ok(0),
    }
}

/// Files a game needs at runtime (tests, golden replays and docs stay home).
fn runtime_file(path: &str, with_tests: bool) -> bool {
    const KEEP: [&str; 10] = [
        ".toml", ".ron", ".lua", ".glb", ".gltf", ".bin", ".png", ".jpg", ".wat", ".wasm",
    ];
    if path.ends_with(".replay.ron") || path.ends_with(".shot.ron") || path.starts_with("target/") {
        return false;
    }
    // Test folders hold scenarios and approved screenshots: only scenarios ship.
    if path.starts_with("tests/") || path.contains("/tests/") {
        return with_tests && path.ends_with(".scenario.ron");
    }
    if path.ends_with(".scenario.ron") {
        return with_tests;
    }
    KEEP.iter().any(|ext| path.ends_with(ext))
}

/// `wp asset build --out`: a copy of the game for one tier, ready to bundle
/// into a phone build. Textures are reduced to the tier's size; game.toml
/// runs that tier; mods that are switched off are left out.
fn ship(
    ctx: &Ctx,
    project: &Project,
    tier: Option<&str>,
    dir: &Path,
    with_tests: bool,
    gpu_compress: bool,
    assets: Vec<serde_json::Value>,
) -> CmdResult {
    let tier = match tier {
        Some(t) => super::replay::parse_tier(t)?,
        None => project.game.value.quality.tier,
    };
    let max_px = tier.max_texture_px();
    if dir.exists() {
        let empty = std::fs::read_dir(dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        if !empty && !dir.join("game.toml").is_file() {
            return Err(cli_error(
                format!("{} exists and is not a game build", dir.display()),
                "choose an empty folder; an earlier build (with a game.toml) is replaced",
            ));
        }
        std::fs::remove_dir_all(dir).map_err(|e| cli_error(e.to_string(), "check permissions"))?;
    }
    let inactive: Vec<String> = project
        .mods
        .iter()
        .filter(|m| !m.active)
        .map(|m| format!("mods/{}/", m.id))
        .collect();
    let files: Vec<String> = waypoint::data::find_files(&project.root, "")
        .into_iter()
        .filter(|f| runtime_file(f, with_tests) && !inactive.iter().any(|m| f.starts_with(m)))
        .collect();
    let (mut bytes_in, mut bytes_out, mut resized) = (0usize, 0usize, 0usize);
    for f in &files {
        let src = std::fs::read(project.root.join(f))
            .map_err(|e| cli_error(format!("cannot read {f}: {e}"), "check the file"))?;
        let data = if f == "game.toml" {
            let mut game = project.game.value.clone();
            game.quality.tier = tier;
            game.mods.disabled.clear(); // switched-off mods are not shipped
            toml::to_string(&game)
                .map_err(|e| cli_error(e.to_string(), "report this bug"))?
                .into_bytes()
        } else if f.ends_with(".glb") {
            let (packed, stats) =
                waypoint::asset::pack_glb(&src, max_px, gpu_compress).map_err(|e| {
                    cli_error(
                        format!("{f}: {e}"),
                        "re-export the model as glTF 2.0 (.glb)",
                    )
                })?;
            resized += stats.resized;
            packed
        } else {
            src.clone()
        };
        bytes_in += src.len();
        bytes_out += data.len();
        let to = dir.join(f);
        std::fs::create_dir_all(to.parent().unwrap())
            .map_err(|e| cli_error(e.to_string(), "check permissions"))?;
        std::fs::write(&to, &data).map_err(|e| cli_error(e.to_string(), "check permissions"))?;
    }
    // The copy must load as cleanly as the original.
    let (_, diags) = Project::load(dir);
    if diags.has_errors() {
        return Err(diags.0);
    }
    let tier_name = super::replay::tier_name(tier);
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "path": dir,
            "tier": tier_name,
            "files": files.len(),
            "bytes_in": bytes_in,
            "bytes_out": bytes_out,
            "textures_resized": resized,
            "max_texture_px": max_px,
            "gpu_compressed": gpu_compress,
            "assets": assets,
        }));
    } else {
        println!(
            "wrote {} for the {tier_name} tier: {} files, {:.2} MB → {:.2} MB ({resized} textures reduced to ≤ {max_px} px)",
            dir.display(),
            files.len(),
            bytes_in as f64 / 1e6,
            bytes_out as f64 / 1e6
        );
    }
    Ok(0)
}
