//! Mods: folders under `mods/` that add files to a game or replace its
//! files, and the layered file system the project loader reads through.
//!
//! A mod mirrors the project's layout: `mods/night_bus/vehicles/x.vehicle.ron`
//! adds a vehicle, `mods/night_bus/scenes/town.scene.ron` replaces the game's
//! `scenes/town.scene.ron`. Mods load in `priority` order (then by folder
//! name); a later mod wins over an earlier one and every mod wins over the
//! game.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wp_core::Diagnostic;
use wp_data::{Loaded, find_files, load_toml};

/// Folder that holds mods, under the project root.
pub const MODS_DIR: &str = "mods";

/// `mods/<folder>/mod.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModManifest {
    /// Shown in `wp mod list`; the folder name is the mod's id.
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// Load order: lower first; later mods override earlier ones.
    #[serde(default)]
    pub priority: i32,
    /// Off: the mod is ignored (as is a mod listed in game.toml [mods] disabled).
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// A mod found under `mods/`.
#[derive(Clone, Debug)]
pub struct Mod {
    /// Folder name.
    pub id: String,
    pub manifest: Loaded<ModManifest>,
    /// Whether it is part of the layered file system.
    pub active: bool,
    /// Files it adds or replaces, by project path.
    pub files: Vec<String>,
}

/// The project's files seen through its active mods.
#[derive(Clone, Debug)]
pub struct Vfs {
    root: PathBuf,
    /// Path prefixes, base first: "", "mods/a/", "mods/b/".
    layers: Vec<String>,
}

impl Vfs {
    /// Read `mods/*/mod.toml` and build the layers. `disabled` lists mod
    /// folders switched off in game.toml.
    pub fn open(root: &Path, disabled: &[String]) -> (Vfs, Vec<Mod>, Vec<Diagnostic>) {
        let mut diags = Vec::new();
        let mut mods = Vec::new();
        let dir = root.join(MODS_DIR);
        let mut folders: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        folders.sort();
        for id in folders {
            let manifest_path = format!("{MODS_DIR}/{id}/mod.toml");
            if !root.join(&manifest_path).is_file() {
                diags.push(
                    Diagnostic::error("WP-X001", format!("mod `{id}` has no mod.toml"))
                        .in_file(format!("{MODS_DIR}/{id}"))
                        .help("create it with `wp mod new <name>`, or add name and version to mods/<name>/mod.toml"),
                );
                continue;
            }
            match load_toml::<ModManifest>(root, &manifest_path) {
                Ok(m) => {
                    let active = m.value.enabled && !disabled.contains(&id);
                    mods.push(Mod {
                        files: Vec::new(),
                        id,
                        manifest: m,
                        active,
                    })
                }
                Err(d) => diags.push(d),
            }
        }
        for d in disabled {
            if !mods.iter().any(|m| &m.id == d) {
                diags.push(
                    Diagnostic::warning(
                        "WP-X002",
                        format!("game.toml disables mod `{d}`, which is not in mods/"),
                    )
                    .in_file("game.toml")
                    .help("remove it from [mods] disabled, or check the folder name"),
                );
            }
        }
        mods.sort_by(|a, b| {
            (a.manifest.value.priority, &a.id).cmp(&(b.manifest.value.priority, &b.id))
        });
        let mut layers = vec![String::new()];
        layers.extend(
            mods.iter()
                .filter(|m| m.active)
                .map(|m| format!("{MODS_DIR}/{}/", m.id)),
        );
        let vfs = Vfs {
            root: root.to_path_buf(),
            layers,
        };
        for m in &mut mods {
            let prefix = format!("{MODS_DIR}/{}", m.id);
            m.files = find_files(&root.join(&prefix), "")
                .into_iter()
                .filter(|f| f != "mod.toml" && f != "README.md")
                .collect();
        }
        (vfs, mods, diags)
    }

    /// Files ending in `suffix`: (project path, real path from the root).
    /// A mod's file replaces a file at the same project path.
    pub fn files(&self, suffix: &str) -> Vec<(String, String)> {
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        for prefix in &self.layers {
            let found = if prefix.is_empty() {
                find_files(&self.root, suffix)
                    .into_iter()
                    .filter(|f| !f.starts_with(&format!("{MODS_DIR}/")))
                    .collect::<Vec<_>>()
            } else {
                find_files(&self.root.join(prefix.trim_end_matches('/')), suffix)
            };
            for f in found {
                out.insert(f.clone(), format!("{prefix}{f}"));
            }
        }
        out.into_iter().collect()
    }

    /// Whether the game itself (no mod) has project path `path`.
    pub fn in_base(&self, path: &str) -> bool {
        !path.starts_with(&format!("{MODS_DIR}/")) && self.root.join(path).is_file()
    }

    /// Where project path `path` lives: the topmost layer that has it.
    pub fn real(&self, path: &str) -> Option<String> {
        self.layers
            .iter()
            .rev()
            .map(|prefix| format!("{prefix}{path}"))
            .find(|p| self.root.join(p).is_file())
    }

    /// Read project path `path` through the layers.
    pub fn read(&self, path: &str) -> Option<(String, String)> {
        let real = self.real(path)?;
        let text = std::fs::read_to_string(self.root.join(&real)).ok()?;
        Some((real, text))
    }
}
