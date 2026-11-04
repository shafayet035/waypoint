//! Assets: models, their licences and their budgets.
//!
//! Artists export glTF 2.0 (`.glb`) into a project's `assets/` folder. Each
//! file has a licence file next to it with the same stem
//! (`assets/vehicles/hatchback.glb` → `assets/vehicles/hatchback.asset.ron`);
//! `wp check` rejects an asset without one and `wp credits` lists them.
//!
//! [`load_model`] reads a model into the engine's frame (metres, X forward,
//! Y left, Z up), one [`Part`] per material. Material names give parts a
//! [`Role`]: `paint…` takes the vehicle's colour, `brake…`/`tail…` light up
//! when braking, `head…` are headlamps, `glass…` is glass.

pub mod basis;
mod lod;
mod meta;
mod model;
mod pack;

pub use lod::{LOD_RATIOS, simplify};
pub use meta::{AssetMeta, Licence, meta_path};
pub use model::{Material, Model, Part, Role, Texture, load_model, parse_model};
pub use pack::{PackStats, pack_glb};

use std::sync::Arc;

/// Model file extensions the engine reads.
pub const MODEL_EXTS: [&str; 2] = [".glb", ".gltf"];

/// Starting triangle budgets per quality tier (Low, Mid, Flagship): the
/// design's table, checked by `wp check`. Phones set the final numbers.
pub const PLAYER_TRIANGLES: [u32; 3] = [40_000, 80_000, 150_000];
/// Nearest-LOD budget for a vehicle used in traffic.
pub const TRAFFIC_TRIANGLES: [u32; 3] = [8_000, 15_000, 30_000];

/// Loaded models by project path, in path order. A model's index is stable
/// for the library's lifetime, so draw lists can refer to it cheaply.
#[derive(Clone, Debug, Default)]
pub struct ModelLibrary {
    entries: Vec<(String, Arc<Model>)>,
}

impl ModelLibrary {
    pub fn new(models: impl IntoIterator<Item = (String, Arc<Model>)>) -> Self {
        let mut entries: Vec<_> = models.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| a.0 == b.0);
        Self { entries }
    }

    /// Index and model for a project path.
    pub fn find(&self, path: &str) -> Option<(u32, &Arc<Model>)> {
        let i = self
            .entries
            .binary_search_by(|(p, _)| p.as_str().cmp(path))
            .ok()?;
        Some((i as u32, &self.entries[i].1))
    }

    pub fn get(&self, index: u32) -> Option<&Arc<Model>> {
        self.entries.get(index as usize).map(|(_, m)| m)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Arc<Model>)> {
        self.entries.iter().map(|(p, m)| (p.as_str(), m))
    }
}

/// World resource: the models the session's vehicles use. Only rendering
/// reads it; it is not part of the simulation state.
#[derive(Clone, Debug, Default)]
pub struct Models(pub Arc<ModelLibrary>);
