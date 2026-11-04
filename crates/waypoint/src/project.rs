//! Loading and checking a project folder.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wp_core::{Diagnostic, Diagnostics};
use wp_data::{Loaded, find_files, load_ron, load_toml};
use wp_road::RoadNetworkDef;
use wp_test::ScenarioDef;
use wp_vehicle::{TrailerDef, VehicleDef};
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
    #[serde(default)]
    pub dev: DevConfig,
}

/// Development-only settings. Leave empty in shipped games.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DevConfig {
    /// If set, `waypoint-player` accepts control-protocol requests on
    /// 127.0.0.1:<port> (on Android, reach it with `adb forward tcp:<port> tcp:<port>`).
    #[serde(default)]
    pub control_port: Option<u16>,
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
    /// By trailer name.
    pub trailers: BTreeMap<String, Loaded<TrailerDef>>,
    /// Road networks by path relative to the root.
    pub roads: BTreeMap<String, Loaded<RoadNetworkDef>>,
    /// Files that exist but failed to parse, so references to them can say so.
    pub failed: Vec<String>,
}

pub const VEHICLE_EXT: &str = ".vehicle.ron";
pub const SCENE_EXT: &str = ".scene.ron";
pub const SCENARIO_EXT: &str = ".scenario.ron";
pub const TRAILER_EXT: &str = ".trailer.ron";
pub const ROAD_EXT: &str = ".road.ron";

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

        let mut trailers = BTreeMap::new();
        for path in find_files(root, TRAILER_EXT) {
            match load_ron::<TrailerDef>(root, &path).inspect_err(|_| failed.push(path.clone())) {
                Ok(t) => {
                    diags.extend(TrailerDef::validate(&t));
                    trailers.insert(t.value.name.clone(), t);
                }
                Err(d) => diags.push(d),
            }
        }

        let mut roads = BTreeMap::new();
        for path in find_files(root, ROAD_EXT) {
            match load_ron::<RoadNetworkDef>(root, &path).inspect_err(|_| failed.push(path.clone()))
            {
                Ok(r) => {
                    diags.extend(RoadNetworkDef::validate(&r));
                    roads.insert(path, r);
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
            trailers,
            roads,
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
        for scene in self.scenes.values() {
            if let Some(path) = &scene.value.roads
                && !self.roads.contains_key(path)
            {
                let why = if self.failed.contains(path) {
                    "failed to load"
                } else {
                    "does not exist"
                };
                out.push(
                    scene
                        .error_at("WP-W005", "roads", format!("road network `{path}` {why}"))
                        .help(format!("known road files: {}", list(self.roads.keys()))),
                );
            }
        }
        for scene in self.scenes.values() {
            for p in &scene.value.parked {
                if !self.vehicles.contains_key(&p.vehicle) {
                    out.push(
                        scene
                            .error_at(
                                "WP-W008",
                                "parked",
                                format!("parked vehicle `{}` does not exist", p.vehicle),
                            )
                            .help(format!("known vehicles: {}", list(self.vehicles.keys()))),
                    );
                }
            }
            let Some(traffic) = &scene.value.traffic else {
                continue;
            };
            if scene.value.roads.is_none() {
                out.push(
                    scene
                        .error_at(
                            "WP-W007",
                            "traffic",
                            "traffic needs a road network to drive on",
                        )
                        .help("add `roads: Some(\"roads/<name>.road.ron\")` or remove `traffic`"),
                );
            }
            for v in &traffic.vehicles {
                if !self.vehicles.contains_key(v) {
                    out.push(
                        scene
                            .error_at(
                                "WP-W006",
                                "vehicles",
                                format!("traffic vehicle `{v}` does not exist"),
                            )
                            .help(format!("known vehicles: {}", list(self.vehicles.keys()))),
                    );
                }
            }
            if traffic.count > 0 && traffic.vehicles.is_empty() {
                out.push(
                    scene
                        .error_at(
                            "WP-W006",
                            "vehicles",
                            "traffic has no vehicles to choose from",
                        )
                        .help("list vehicle names, e.g. vehicles: [\"hatchback\"]"),
                );
            }
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
            if let wp_test::Driver::ReverseToDock { dock, .. } = &s.player.driver {
                if s.player.trailer.is_none() {
                    out.push(
                        sc.error_at(
                            "WP-S021",
                            "driver",
                            "ReverseToDock needs a trailer to back in",
                        )
                        .help("add trailer: Some(\"semi_trailer\") to the player"),
                    );
                }
                if let Some(scene) = self.scenes.get(&s.scene)
                    && scene.value.dock(dock).is_none()
                {
                    out.push(
                        sc.error_at(
                            "WP-S020",
                            "dock",
                            format!("dock `{dock}` is not in {}", s.scene),
                        )
                        .help(format!(
                            "docks there: {}",
                            list(scene.value.docks.iter().map(|d| &d.name))
                        )),
                    );
                }
            }
            if let Some(t) = &s.player.trailer {
                if !self.trailers.contains_key(t) {
                    out.push(
                        sc.error_at(
                            "WP-S018",
                            "trailer",
                            format!("trailer `{t}` does not exist"),
                        )
                        .help(format!("known trailers: {}", list(self.trailers.keys()))),
                    );
                } else if self
                    .vehicles
                    .get(&s.player.vehicle)
                    .is_some_and(|v| v.value.hitch.is_none())
                {
                    out.push(
                        sc.error_at("WP-S019", "trailer", format!("vehicle `{}` has no fifth wheel for a trailer", s.player.vehicle))
                            .help("use a tractor unit, or add hitch: Some((ahead_of_rear_axle_m: 0.5)) to the vehicle"),
                    );
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

    /// Everything needed to run scenario `name`. `None` if it or what it
    /// refers to is missing (`wp check` reports why).
    pub fn scenario_input<'a>(&'a self, name: &'a str) -> Option<wp_test::ScenarioInput<'a>> {
        let s = &self.scenarios.get(name)?.value;
        let trailer = match &s.player.trailer {
            Some(t) => Some(&self.trailers.get(t)?.value),
            None => None,
        };
        let scene = &self.scenes.get(&s.scene)?.value;
        Some(wp_test::ScenarioInput {
            name,
            scenario: s,
            roads: self.scene_roads(scene),
            traffic: self.scene_vehicles(scene)?,
            scene,
            vehicle: &self.vehicles.get(&s.player.vehicle)?.value,
            trailer,
        })
    }

    /// The vehicles a scene uses for traffic and parked vehicles, once each.
    /// `None` if one is missing (`wp check` reports it).
    pub fn scene_vehicles(&self, scene: &SceneDef) -> Option<Vec<&wp_vehicle::VehicleDef>> {
        let mut names: Vec<&String> = scene.parked.iter().map(|p| &p.vehicle).collect();
        if let Some(t) = &scene.traffic {
            names.extend(&t.vehicles);
        }
        names.sort();
        names.dedup();
        names
            .into_iter()
            .map(|v| self.vehicles.get(v).map(|l| &l.value))
            .collect()
    }

    /// The road network a scene refers to, if any (and if it loaded).
    pub fn scene_roads(&self, scene: &SceneDef) -> Option<&RoadNetworkDef> {
        scene
            .roads
            .as_ref()
            .and_then(|p| self.roads.get(p))
            .map(|r| &r.value)
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
