//! Loading and checking a project folder.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wp_core::{Diagnostic, Diagnostics};
use wp_data::{Loaded, load_ron, load_toml};
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
    #[serde(default)]
    pub quality: QualityConfig,
    #[serde(default)]
    pub mods: ModsConfig,
    /// Art style: a preset (realistic, toon, flat) and colour overrides.
    #[serde(default)]
    pub look: wp_world::LookDef,
}

/// Mod settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModsConfig {
    /// Mod folders under mods/ to ignore.
    #[serde(default)]
    pub disabled: Vec<String>,
}

/// Quality settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QualityConfig {
    /// low, mid or flagship (render resolution, traffic and effects).
    #[serde(default)]
    pub tier: wp_world::QualityTier,
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
    /// Mods under mods/, in load order (active or not).
    pub mods: Vec<crate::mods::Mod>,
    /// The game's files seen through its active mods.
    pub vfs: crate::mods::Vfs,
    /// Active mods' WebAssembly modules, compiled.
    pub wasm_modules: Vec<wp_wasm::ModuleDef>,
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
    /// By mission name (file name without `.mission.ron`).
    pub missions: BTreeMap<String, Loaded<wp_mission::MissionDef>>,
    /// Licence files of assets, by the asset's path.
    pub assets: BTreeMap<String, Loaded<wp_asset::AssetMeta>>,
    /// Every model in the project that loaded, by path.
    pub models: std::sync::Arc<wp_asset::ModelLibrary>,
    /// Recorded replays (`wp replay`), by path. `tests/<name>.replay.ron` is
    /// the golden replay `wp test` compares scenario `<name>` with.
    pub replays: BTreeMap<String, Loaded<wp_vehicle::replay::Replay>>,
    /// Files that exist but failed to parse, so references to them can say so.
    pub failed: Vec<String>,
}

pub const VEHICLE_EXT: &str = ".vehicle.ron";
pub const SCENE_EXT: &str = ".scene.ron";
pub const SCENARIO_EXT: &str = ".scenario.ron";
pub const TRAILER_EXT: &str = ".trailer.ron";
pub const ROAD_EXT: &str = ".road.ron";
pub const MISSION_EXT: &str = ".mission.ron";
pub const REPLAY_EXT: &str = ".replay.ron";

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

        let (vfs, mods, mod_diags) = crate::mods::Vfs::open(root, &game.value.mods.disabled);
        diags.extend(mod_diags);
        let (wasm_modules, wasm_diags) = crate::mods::Vfs::wasm_modules(root, &mods);
        diags.extend(wasm_diags);
        let mut failed = Vec::new();
        let mut vehicles = BTreeMap::new();
        for (path, real) in vfs.files(VEHICLE_EXT) {
            match load_ron::<VehicleDef>(root, &real).inspect_err(|_| failed.push(path.clone())) {
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
        for (path, real) in vfs.files(TRAILER_EXT) {
            match load_ron::<TrailerDef>(root, &real).inspect_err(|_| failed.push(path.clone())) {
                Ok(t) => {
                    diags.extend(TrailerDef::validate(&t));
                    trailers.insert(t.value.name.clone(), t);
                }
                Err(d) => diags.push(d),
            }
        }

        let mut roads = BTreeMap::new();
        for (path, real) in vfs.files(ROAD_EXT) {
            match load_ron::<RoadNetworkDef>(root, &real).inspect_err(|_| failed.push(path.clone()))
            {
                Ok(r) => {
                    diags.extend(RoadNetworkDef::validate(&r));
                    roads.insert(path, r);
                }
                Err(d) => diags.push(d),
            }
        }

        let mut scenes = BTreeMap::new();
        for (path, real) in vfs.files(SCENE_EXT) {
            match load_ron::<SceneDef>(root, &real).inspect_err(|_| failed.push(path.clone())) {
                Ok(s) => {
                    diags.extend(SceneDef::validate(&s));
                    scenes.insert(path, s);
                }
                Err(d) => diags.push(d),
            }
        }

        let mut scenarios = BTreeMap::new();
        for (path, real) in vfs.files(SCENARIO_EXT) {
            match load_ron::<ScenarioDef>(root, &real) {
                Ok(s) => {
                    diags.extend(ScenarioDef::validate(&s));
                    scenarios.insert(stem(&path, SCENARIO_EXT).to_string(), s);
                }
                Err(d) => diags.push(d),
            }
        }

        let mut missions = BTreeMap::new();
        for (path, real) in vfs.files(MISSION_EXT) {
            match load_ron::<wp_mission::MissionDef>(root, &real) {
                Ok(m) => {
                    missions.insert(stem(&path, MISSION_EXT).to_string(), m);
                }
                Err(d) => diags.push(d),
            }
        }

        let mut models = Vec::new();
        let mut assets = BTreeMap::new();
        for ext in wp_asset::MODEL_EXTS {
            for (path, real) in vfs.files(ext) {
                match wp_asset::load_model(root, &real) {
                    Ok(m) => {
                        if m.skipped > 0 {
                            diags.push(
                                Diagnostic::warning(
                                    "WP-F010",
                                    format!(
                                        "{} primitives are not triangles and are not drawn",
                                        m.skipped
                                    ),
                                )
                                .in_file(path.clone())
                                .help("triangulate the mesh before export (Blender: Triangulate modifier, or export with Apply Modifiers)"),
                            );
                        }
                        models.push((path.clone(), std::sync::Arc::new(m)));
                    }
                    Err(d) => {
                        failed.push(path.clone());
                        diags.push(d);
                    }
                }
                let meta_virtual = wp_asset::meta_path(&path);
                let Some(meta) = vfs.real(&meta_virtual) else {
                    diags.push(
                        Diagnostic::error("WP-F003", "asset has no licence file")
                            .in_file(real.clone())
                            .help(format!(
                                "add {meta_virtual}: (source: \"...\", author: \"...\", licence: Cc0) — see `wp schema asset`"
                            )),
                    );
                    continue;
                };
                match load_ron::<wp_asset::AssetMeta>(root, &meta) {
                    Ok(m) => {
                        if m.value.licence.needs_credit()
                            && m.value
                                .credit
                                .as_deref()
                                .is_none_or(|c| c.trim().is_empty())
                        {
                            diags.push(
                                m.error_at(
                                    "WP-F004",
                                    "licence",
                                    format!("{} needs a credit line", m.value.licence.label()),
                                )
                                .help("add credit: Some(\"<title> by <author>, CC BY 4.0\") for the credits screen"),
                            );
                        }
                        assets.insert(path, m);
                    }
                    Err(d) => diags.push(d),
                }
            }
        }

        let mut replays = BTreeMap::new();
        for (_, real) in vfs.files(REPLAY_EXT) {
            match load_ron::<wp_vehicle::replay::Replay>(root, &real) {
                Ok(r) => {
                    replays.insert(real, r);
                }
                Err(d) => diags.push(d),
            }
        }

        let project = Project {
            wasm_modules,
            mods,
            vfs,
            replays,
            assets,
            models: std::sync::Arc::new(wp_asset::ModelLibrary::new(models)),
            missions,
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
        diags.extend(project.check_models());
        diags.extend(project.check_replays());
        (Some(project), diags)
    }

    /// Vehicle models: present, sized like the vehicle, on the ground and
    /// within the quality tier's triangle budgets.
    fn check_models(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let tier = match self.game.value.quality.tier {
            wp_world::QualityTier::Low => 0,
            wp_world::QualityTier::Mid => 1,
            wp_world::QualityTier::Flagship => 2,
        };
        let in_traffic = |name: &str| {
            self.scenes.values().any(|s| {
                s.value
                    .traffic
                    .as_ref()
                    .is_some_and(|t| t.vehicles.iter().any(|v| v == name))
            })
        };
        for v in self.vehicles.values() {
            let Some(path) = &v.value.model else {
                continue;
            };
            let Some((_, model)) = self.models.find(path) else {
                let why = if self.failed.contains(path) {
                    "failed to load"
                } else {
                    "does not exist"
                };
                out.push(
                    v.error_at("WP-F005", "model", format!("model `{path}` {why}"))
                        .help(format!(
                            "models in the project: {}",
                            list(
                                self.models
                                    .iter()
                                    .map(|(p, _)| p.to_string())
                                    .collect::<Vec<_>>()
                                    .iter()
                            )
                        )),
                );
                continue;
            };
            let fp = wp_vehicle::vehicle_footprint(&v.value);
            let size = model.size();
            let (length, width) = (fp.front - fp.rear, fp.width);
            // Mirrors may stick out up to 0.5 m beyond the body's width.
            let long = (size.x - length).abs() > 0.1 * length;
            let wide = size.y < 0.9 * width || size.y > width + 0.5;
            if long || wide {
                out.push(
                    v.warning_at(
                        "WP-F006",
                        "model",
                        format!(
                            "model is {:.2} × {:.2} m but the vehicle is {length:.2} × {width:.2} m",
                            size.x, size.y
                        ),
                    )
                    .help("model in metres, to the vehicle's body dimensions (set `body` for exact sizes); X forward in the engine is glTF +Z"),
                );
            }
            let mid = (v.value.cg_to_front_axle_m - v.value.rear_axle_from_cg()) / 2.0;
            let model_mid = (model.min.x + model.max.x) / 2.0;
            let body_mid = (fp.front + fp.rear) / 2.0 - mid;
            // Wheels are drawn by the engine, so a body may sit above the ground.
            let r = v.value.wheel.radius_m;
            if model.min.z < -0.05 || model.min.z > r || (model_mid - body_mid).abs() > 0.25 {
                out.push(
                    v.warning_at(
                        "WP-F007",
                        "model",
                        format!(
                            "model origin is off: lowest point at {:.2} m, centre {:.2} m from the axle midpoint (expected {body_mid:.2})",
                            model.min.z, model_mid
                        ),
                    )
                    .help("put the origin on the ground, midway between the front and rear axles"),
                );
            }
            let budget = wp_asset::PLAYER_TRIANGLES[tier];
            if model.triangles > budget {
                out.push(
                    v.error_at(
                        "WP-F008",
                        "model",
                        format!(
                            "model has {} triangles; the {:?} tier allows {budget} for a player vehicle",
                            model.triangles, self.game.value.quality.tier
                        ),
                    )
                    .help("decimate the mesh (Blender: Decimate modifier) or pick a higher [quality] tier"),
                );
            } else if in_traffic(&v.value.name)
                && model.triangles > wp_asset::TRAFFIC_TRIANGLES[tier]
            {
                out.push(
                    v.warning_at(
                        "WP-F008",
                        "model",
                        format!(
                            "model has {} triangles; traffic vehicles at the {:?} tier get {}",
                            model.triangles,
                            self.game.value.quality.tier,
                            wp_asset::TRAFFIC_TRIANGLES[tier]
                        ),
                    )
                    .help("decimate the mesh, or keep this vehicle out of scene traffic"),
                );
            }
            if self.assets.get(path).is_some_and(|a| a.value.generated) {
                out.push(
                    v.warning_at(
                        "WP-F009",
                        "model",
                        "AI-generated models are for background props, not vehicles",
                    )
                    .help("model the vehicle by hand, or use a CC0 or bought model"),
                );
            }
        }
        out
    }

    /// Replays must refer to things that exist.
    fn check_replays(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for r in self.replays.values() {
            let s = &r.value.setup;
            let missing = |what: &str, name: &Option<String>, ok: bool| {
                name.as_ref().filter(|_| !ok).map(|n| {
                    r.error_at("WP-Y001", what, format!("{what} `{n}` does not exist"))
                        .help("re-record the replay, or restore what it refers to")
                })
            };
            out.extend(missing(
                "scenario",
                &s.scenario,
                s.scenario
                    .as_ref()
                    .is_none_or(|n| self.scenarios.contains_key(n)),
            ));
            out.extend(missing(
                "scene",
                &s.scene,
                s.scene.as_ref().is_none_or(|n| self.scenes.contains_key(n)),
            ));
            out.extend(missing(
                "vehicle",
                &s.vehicle,
                s.vehicle
                    .as_ref()
                    .is_none_or(|n| self.vehicles.contains_key(n)),
            ));
            out.extend(missing(
                "mission",
                &s.mission,
                s.mission
                    .as_ref()
                    .is_none_or(|n| self.missions.contains_key(n)),
            ));
        }
        out
    }

    /// The golden replay for a scenario: next to its file, `.replay.ron`.
    pub fn golden_replay(&self, scenario: &str) -> Option<&Loaded<wp_vehicle::replay::Replay>> {
        let path = self
            .scenarios
            .get(scenario)?
            .path
            .replace(SCENARIO_EXT, REPLAY_EXT);
        self.replays.get(&path)
    }

    /// Credits for the assets in use: (asset path, line). CC0 and own work
    /// need none but are listed when they name a credit.
    pub fn credits(&self) -> Vec<(String, String)> {
        self.assets
            .iter()
            .map(|(path, m)| {
                let m = &m.value;
                let line = m.credit.clone().unwrap_or_else(|| {
                    format!(
                        "{} by {} ({})",
                        path.rsplit('/').next().unwrap_or(path),
                        m.author,
                        m.licence.label()
                    )
                });
                (path.clone(), line)
            })
            .collect()
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
        for m in self.missions.values() {
            let d = &m.value;
            let Some(scene) = self.scenes.get(&d.scene) else {
                out.push(
                    m.error_at(
                        "WP-M001",
                        "scene",
                        format!("scene `{}` does not exist", d.scene),
                    )
                    .help(format!("known scenes: {}", list(self.scenes.keys()))),
                );
                continue;
            };
            if !self.vehicles.contains_key(&d.vehicle) {
                out.push(
                    m.error_at(
                        "WP-M002",
                        "vehicle",
                        format!("vehicle `{}` does not exist", d.vehicle),
                    )
                    .help(format!("known vehicles: {}", list(self.vehicles.keys()))),
                );
            }
            if scene.value.spawn(&d.spawn).is_none() {
                out.push(
                    m.error_at(
                        "WP-M003",
                        "spawn",
                        format!("spawn point `{}` is not in {}", d.spawn, d.scene),
                    )
                    .help("use a name from the scene's `spawns`"),
                );
            }
            let roads = self.scene_roads(&scene.value);
            for st in &d.stops {
                if !roads.is_some_and(|r| r.stops.iter().any(|x| x.id == st.stop)) {
                    out.push(
                        m.error_at(
                            "WP-M004",
                            "stops",
                            format!("stop `{}` is not in the scene's road network", st.stop),
                        )
                        .help("add it to the road file's `stops`, or fix the id"),
                    );
                }
            }
            if d.stops.is_empty() {
                out.push(
                    m.error_at("WP-M004", "stops", "the route has no stops")
                        .help("list at least one stop"),
                );
            }
            if let Some(script) = &d.script {
                match self.vfs.read(script).map(|(_, src)| src).ok_or(()) {
                    Err(()) => out.push(
                        m.error_at(
                            "WP-M005",
                            "script",
                            format!("script `{script}` does not exist"),
                        )
                        .help("paths are relative to the project root, e.g. missions/route_1.lua"),
                    ),
                    Ok(source) => {
                        if let Err(e) = wp_script::check(&source, script) {
                            let mut d = Diagnostic::error("WP-L001", format!("{script}: {}", e.message))
                                .help("fix the Lua (Luau) syntax; `wp docs scripting` lists the hooks and API");
                            d.location = Some(wp_core::Location {
                                file: script.clone(),
                                line: e.line.map(|l| l as usize),
                                column: None,
                            });
                            out.push(d);
                        }
                    }
                }
            }
        }
        for sc in self.scenarios.values() {
            let s = &sc.value;
            if let Some(mname) = &s.mission {
                match self.missions.get(mname) {
                    None => out.push(
                        sc.error_at(
                            "WP-S021",
                            "mission",
                            format!("mission `{mname}` does not exist"),
                        )
                        .help(format!("known missions: {}", list(self.missions.keys()))),
                    ),
                    Some(m) if m.value.scene != s.scene => out.push(
                        sc.error_at(
                            "WP-S021",
                            "mission",
                            format!(
                                "mission `{mname}` is set in {}, not {}",
                                m.value.scene, s.scene
                            ),
                        )
                        .help("use the mission's scene for the scenario"),
                    ),
                    _ => {}
                }
            }
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
            mission: match &s.mission {
                Some(m) => Some(&self.missions.get(m)?.value),
                None => None,
            },
            mission_script: s
                .mission
                .as_ref()
                .and_then(|m| self.missions.get(m))
                .and_then(|m| self.mission_script(&m.value)),
            scene,
            vehicle: &self.vehicles.get(&s.player.vehicle)?.value,
            trailer,
            wasm: &self.wasm_modules,
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

    /// A mission's script: (path, source), if it has one that can be read.
    pub fn mission_script(&self, def: &wp_mission::MissionDef) -> Option<(String, String)> {
        let path = def.script.as_ref()?;
        self.vfs.read(path)
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
