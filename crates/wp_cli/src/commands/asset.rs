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
    if model.textures > 0 {
        println!(
            "  {} textures (not drawn yet; materials use their colour factors)",
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
