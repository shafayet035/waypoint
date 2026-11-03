//! Loading and checking a project folder.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wp_core::{Diagnostic, Diagnostics};
use wp_data::{Loaded, find_files, load_ron, load_toml};
use wp_test::ScenarioDef;
use wp_vehicle::VehicleDef;
use wp_world::SceneDef;

/// `game.toml` at the project root.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GameConfig {
    pub name: String,
    pub version: String,
    /// Scene loaded by `wp run`, relative to the project root.
    pub start_scene: String,
    #[serde(default)]
    pub plugins: PluginsConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginsConfig {
    /// Engine plugins to switch off, by name (see `wp plugins`).
    #[serde(default)]
    pub disabled: Vec<String>,
}

pub struct Project {
    pub root: PathBuf,
    pub game: Loaded<GameConfig>,
    /// By vehicle name.
    pub vehicles: BTreeMap<String, Loaded<VehicleDef>>,
    /// By path relative to the root.
    pub scenes: BTreeMap<String, Loaded<SceneDef>>,
    /// By scenario name (file name without `.scenario.ron`).
    pub scenarios: BTreeMap<String, Loaded<ScenarioDef>>,
    /// Files that exist but failed to parse, so references to them can say so.
    pub failed: Vec<String>,
}

pub const VEHICLE_EXT: &str = ".vehicle.ron";
pub const SCENE_EXT: &str = ".scene.ron";
pub const SCENARIO_EXT: &str = ".scenario.ron";

fn stem<'a>(path: &'a str, ext: &str) -> &'a str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(ext).unwrap_or(file)
}

impl Project {
    /// Load and validate everything. Returns the project when `game.toml`
    /// parses, plus every diagnostic found (errors and warnings).
    pub fn load(root: &Path) -> (Option<Project>, Diagnostics) {
        let mut diags = Diagnostics::default();
        if !root.join("game.toml").is_file() {
            diags.push(
                Diagnostic::error("WP-G001", format!("no game.toml in {}", root.display()))
                    .help("run inside a Waypoint project, pass --project <dir>, or create one with `wp new <name>`"),
            );
            return (None, diags);
        }
        let game = match load_toml::<GameConfig>(root, "game.toml") {
            Ok(g) => g,
            Err(d) => {
                diags.push(d);
                return (None, diags);
            }
        };

        let mut failed = Vec::new();
        let mut vehicles = BTreeMap::new();
        for path in find_files(root, VEHICLE_EXT) {
            match load_ron::<VehicleDef>(root, &path).inspect_err(|_| failed.push(path.clone())) {
                Ok(v) => {
                    diags.extend(VehicleDef::validate(&v));
                    if v.value.name != stem(&path, VEHICLE_EXT) {
                        diags.push(
                            v.warning_at(
                                "WP-V002",
                                "name",
                                format!(
                                    "vehicle name `{}` does not match its file name `{}`",
                                    v.value.name,
                                    stem(&path, VEHICLE_EXT)
                                ),
                            )
                            .help("rename the file or the vehicle so scenarios are easy to follow"),
                        );
                    }
                    if let Some(prev) = vehicles.get(&v.value.name) {
                        let prev: &Loaded<VehicleDef> = prev;
                        diags.push(
                            v.error_at(
                                "WP-V004",
                                "name",
                                format!("vehicle name `{}` is already used", v.value.name),
                            )
                            .help(format!("first defined in {}", prev.path)),
                        );
                    } else {
                        vehicles.insert(v.value.name.clone(), v);
                    }
                }
                Err(d) => diags.push(d),
            }
        }

        let mut scenes = BTreeMap::new();
        for path in find_files(root, SCENE_EXT) {
            match load_ron::<SceneDef>(root, &path).inspect_err(|_| failed.push(path.clone())) {
                Ok(s) => {
                    diags.extend(SceneDef::validate(&s));
                    scenes.insert(path, s);
                }
                Err(d) => diags.push(d),
            }
        }

        let mut scenarios = BTreeMap::new();
        for path in find_files(root, SCENARIO_EXT) {
            match load_ron::<ScenarioDef>(root, &path) {
                Ok(s) => {
                    diags.extend(ScenarioDef::validate(&s));
                    scenarios.insert(stem(&path, SCENARIO_EXT).to_string(), s);
                }
                Err(d) => diags.push(d),
            }
        }

        let project = Project {
            root: root.to_path_buf(),
            game,
            vehicles,
            scenes,
            scenarios,
            failed,
        };
        diags.extend(project.check_references());
        (Some(project), diags)
    }

    /// Cross-file checks: every name a file refers to must exist.
    fn check_references(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let start = &self.game.value.start_scene;
        if !self.scenes.contains_key(start) {
            out.push(
                self.game
                    .error_at(
                        "WP-G010",
                        "start_scene",
                        format!("start_scene `{start}` does not exist"),
                    )
                    .help(format!("known scenes: {}", list(self.scenes.keys()))),
            );
        }
        for sc in self.scenarios.values() {
            let s = &sc.value;
            match self.scenes.get(&s.scene) {
                None if self.failed.contains(&s.scene) => out.push(
                    sc.error_at(
                        "WP-S010",
                        "scene",
                        format!("scene `{}` failed to load", s.scene),
                    )
                    .help("fix the errors in that file first"),
                ),
                None => out.push(
                    sc.error_at(
                        "WP-S010",
                        "scene",
                        format!("scene `{}` does not exist", s.scene),
                    )
                    .help(format!("known scenes: {}", list(self.scenes.keys()))),
                ),
                Some(scene) => {
                    if scene.value.spawn(&s.player.spawn).is_none() {
                        out.push(
                            sc.error_at(
                                "WP-S012",
                                "spawn",
                                format!("spawn point `{}` is not in {}", s.player.spawn, s.scene),
                            )
                            .help(format!(
                                "spawn points there: {}",
                                list(scene.value.spawns.iter().map(|p| &p.name))
                            )),
                        )
                    }
                }
            }
            let vehicle_file = format!("{}{VEHICLE_EXT}", s.player.vehicle);
            if !self.vehicles.contains_key(&s.player.vehicle)
                && let Some(f) = self
                    .failed
                    .iter()
                    .find(|f| f.ends_with(&format!("/{vehicle_file}")))
            {
                out.push(
                    sc.error_at(
                        "WP-S011",
                        "vehicle",
                        format!("vehicle `{}` failed to load", s.player.vehicle),
                    )
                    .help(format!("fix the errors in {f} first")),
                );
            } else if !self.vehicles.contains_key(&s.player.vehicle) {
                out.push(
                    sc.error_at(
                        "WP-S011",
                        "vehicle",
                        format!("vehicle `{}` does not exist", s.player.vehicle),
                    )
                    .help(format!("known vehicles: {}", list(self.vehicles.keys()))),
                );
            }
        }
        out
    }

    pub fn disabled_plugins(&self) -> &[String] {
        &self.game.value.plugins.disabled
    }
}

fn list<'a>(items: impl Iterator<Item = &'a String>) -> String {
    let v: Vec<&str> = items.map(|s| s.as_str()).collect();
    if v.is_empty() {
        "(none)".into()
    } else {
        v.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sandbox")
    }

    #[test]
    fn sandbox_loads_cleanly() {
        let (p, d) = Project::load(&sandbox());
        assert!(
            d.is_empty(),
            "{}",
            d.iter().map(|d| d.render()).collect::<String>()
        );
        let p = p.unwrap();
        assert!(p.vehicles.contains_key("hatchback"));
        assert!(p.scenarios.contains_key("skidpad"));
    }

    #[test]
    fn missing_game_toml() {
        let (p, d) = Project::load(Path::new("/definitely/not/here"));
        assert!(p.is_none());
        assert_eq!(d.iter().next().unwrap().code, "WP-G001");
    }
}
