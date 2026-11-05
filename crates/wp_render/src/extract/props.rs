//! Drawing scenery models: props (benches, bins, rocks), model trees, and
//! the models a game gives the engine's own street furniture and people
//! (`scenery::FurnitureDef`). Each model part is one draw item in its
//! material's colours; parts whose material is named `head…` glow when lit.

#[allow(unused_imports)]
use super::*;
use wp_asset::{ModelLibrary, Role};
use wp_world::scenery::Scenery;

/// Props within this distance of the focus are drawn, m.
const PROP_DRAW_M: f32 = 300.0;
/// How brightly a lit lamp head glows.
const LAMP_GLOW: f32 = 6.0;

/// Push the parts of model `path` at `transform`. False if the library
/// does not have it (the caller falls back to built-in shapes).
pub(super) fn model_items(
    items: &mut Vec<DrawItem>,
    models: Option<&ModelLibrary>,
    path: &str,
    transform: Mat4,
    lit: bool,
) -> bool {
    let Some((index, model)) = models.and_then(|m| m.find(path)) else {
        return false;
    };
    for (k, part) in model.parts.iter().enumerate() {
        let m = &part.material;
        let emissive = match m.role {
            Role::HeadLight if lit => LAMP_GLOW,
            _ => m.emissive,
        };
        items.push(DrawItem {
            mesh: MeshId::Model {
                model: index,
                part: k as u32,
                lod: 0,
            },
            transform,
            color: Vec3::from(m.base_color),
            roughness: m.roughness,
            metallic: m.metallic,
            emissive,
        });
    }
    true
}

/// Placed props near the focus.
pub(super) fn prop_items(
    items: &mut Vec<DrawItem>,
    scenery: &Scenery,
    models: Option<&ModelLibrary>,
    focus: Vec2,
) {
    for p in &scenery.props {
        if p.position.distance(focus) > PROP_DRAW_M {
            continue;
        }
        let at = Mat4::from_translation(p.position.extend(0.0))
            * Mat4::from_rotation_z(p.heading)
            * Mat4::from_scale(Vec3::splat(p.scale));
        model_items(items, models, &p.model, at, false);
    }
}

/// A model tree `t`, scaled to its height and turned by its seed. False if
/// the model is missing.
pub(super) fn model_tree(
    items: &mut Vec<DrawItem>,
    models: Option<&ModelLibrary>,
    t: &wp_world::scenery::Tree,
) -> bool {
    let Some(path) = &t.model else { return false };
    let Some((_, model)) = models.and_then(|m| m.find(path)) else {
        return false;
    };
    let scale = t.height_m / model.size().z.max(0.1);
    let at = Mat4::from_translation(t.position.extend(0.0))
        * Mat4::from_rotation_z(t.seed as f32 * 0.7)
        * Mat4::from_scale(Vec3::splat(scale));
    model_items(items, models, path, at, false)
}
