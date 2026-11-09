//! Scenes. M0 scenes are a flat ground plane with named spawn points; roads,
//! tiles and streaming arrive with `wp_road` and world streaming in M2–M3.

pub mod grass;
mod look;
pub mod scenery;
pub mod terrain;
use glam::Vec2;
pub use look::{ArtStyle, Look, LookDef, srgb};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wp_app::{App, Plugin};
use wp_core::Diagnostic;
use wp_data::Loaded;
use wp_ecs::{Component, World};
use wp_physics::{Gravity, Ground};

/// A scene file: `scenes/<name>.scene.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneDef {
    pub name: String,
    #[serde(default)]
    pub ground: GroundDef,
    /// Named places where vehicles can start.
    pub spawns: Vec<SpawnPoint>,
    /// Loading docks a trailer can be reversed into.
    #[serde(default)]
    pub docks: Vec<DockDef>,
    /// Road network file (`roads/<name>.road.ron`), relative to the project root.
    #[serde(default)]
    pub roads: Option<String>,
    /// Buildings (`scenery/<name>.scenery.ron`), relative to the project root.
    #[serde(default)]
    pub scenery: Option<String>,
    /// AI traffic on the road network.
    #[serde(default)]
    pub traffic: Option<TrafficDef>,
    /// Vehicles standing still in the scene, brakes on (obstacles, set dressing).
    #[serde(default)]
    pub parked: Vec<ParkedDef>,
    /// People walking the sidewalks (roads need `sidewalk_m` > 0).
    #[serde(default)]
    pub pedestrians: Option<PedestrianDef>,
    /// Rain and fog. Wet roads have less grip, fog hides what is ahead;
    /// drivers slow down for both.
    #[serde(default)]
    pub weather: Option<WeatherDef>,
    /// Time of day (sun, sky, headlights, street lamps). Absent: early afternoon.
    #[serde(default)]
    pub time_of_day: Option<TimeOfDayDef>,
    /// Hills and valleys (absent: flat ground). Roads are levelled into it.
    #[serde(default)]
    pub terrain: Option<terrain::TerrainDef>,
    /// Lakes, ponds and fords: each fills low land up to its level (needs
    /// `terrain`).
    #[serde(default)]
    pub water: Vec<terrain::WaterDef>,
    /// Dense grass that sways in the wind, drawn near the camera (kept off
    /// roads, sidewalks, junctions, buildings and steep ground).
    #[serde(default)]
    pub grass: Option<grass::GrassDef>,
}

/// A scene's clock.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeOfDayDef {
    /// Hour at the start, 0–24 (21.5 is half past nine in the evening).
    pub hour: f32,
    /// Game hours per real hour: 0 stands still, 60 runs a day in 24 minutes.
    #[serde(default)]
    pub hours_per_hour: f32,
}

/// The time now (`world.set` on "TimeOfDay" changes it live).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TimeOfDay {
    /// 0–24.
    pub hour: f32,
    /// Game hours per simulated hour.
    pub rate: f32,
}

/// How much detail the game runs with. Phones are grouped into three tiers
/// (the design's table); the engine scales render resolution, traffic,
/// pedestrians and effects to suit.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, PartialOrd, Ord, Eq, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    /// Older or budget phones: 30 fps target, upscaled from about 540p.
    Low,
    /// Mainstream phones: 60 fps, upscaled from about 720p.
    #[default]
    Mid,
    /// Current flagships: 60 fps, upscaled from about 900p.
    Flagship,
}

impl QualityTier {
    /// Highest internal rendering height; taller screens are upscaled.
    pub fn max_height(self) -> u32 {
        match self {
            Self::Low => 540,
            Self::Mid => 720,
            Self::Flagship => 900,
        }
    }
    /// Largest texture side, px (the design's budget table).
    pub fn max_texture_px(self) -> u32 {
        match self {
            Self::Low => 1024,
            Self::Mid => 2048,
            Self::Flagship => 4096,
        }
    }
    /// How far models keep full detail (multiplies the level-of-detail distances).
    pub fn lod_bias(self) -> f32 {
        match self {
            Self::Low => 0.6,
            Self::Mid => 1.0,
            Self::Flagship => 1.6,
        }
    }
    /// Street reflections in wet roads (a mirrored pass at half size).
    /// Share of a scene's grass reach drawn.
    pub fn grass_reach(self) -> f32 {
        match self {
            QualityTier::Low => 0.45,
            QualityTier::Mid => 0.75,
            QualityTier::Flagship => 1.0,
        }
    }
    pub fn reflections(self) -> bool {
        self != Self::Low
    }
    /// Local lights (headlamps, street lamps) shaded per frame, nearest first.
    pub fn max_lights(self) -> usize {
        match self {
            Self::Low => 16,
            Self::Mid => 32,
            Self::Flagship => 64,
        }
    }
    /// Sun shadow map: texels per side and the half-width it covers, m.
    pub fn shadow_map(self) -> (u32, f32) {
        match self {
            Self::Low => (1024, 30.0),
            Self::Mid => (2048, 45.0),
            Self::Flagship => (4096, 60.0),
        }
    }
    /// How far road tiles stream in around the camera, m (fog hides the edge).
    pub fn view_distance_m(self) -> f32 {
        match self {
            Self::Low => 700.0,
            Self::Mid => 1000.0,
            Self::Flagship => 1500.0,
        }
    }
    /// Multiplier on a scene's traffic count.
    pub fn traffic_scale(self) -> f32 {
        match self {
            Self::Low => 0.6,
            Self::Mid => 1.0,
            Self::Flagship => 1.3,
        }
    }
    /// Multiplier on a scene's pedestrian count.
    pub fn pedestrian_scale(self) -> f32 {
        match self {
            Self::Low => 0.5,
            Self::Mid => 1.0,
            Self::Flagship => 1.5,
        }
    }
    /// Multiplier on rain streaks drawn.
    pub fn rain_density(self) -> f32 {
        match self {
            Self::Low => 0.5,
            _ => 1.0,
        }
    }

    /// The name game.toml and the command line use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Mid => "mid",
            Self::Flagship => "flagship",
        }
    }

    /// The tier a device can hold. `bench_ms`: the renderer benchmark's mean
    /// frame time at Mid settings and 1280×720 (`wp bench`); `memory_gb`: the
    /// device's RAM, if known; `software`: no GPU, a CPU rasteriser.
    ///
    /// Mid must hold 60 fps with room for the simulation, so its benchmark
    /// frame must take at most 12 ms; Flagship draws about twice as much
    /// (900 lines, twice the lights, a 4096 shadow map), so at most 6 ms.
    /// Memory caps it as the design's device classes do: under 5 GB (a 4 GB
    /// phone) Low, under 7 GB (6 GB) at most Mid.
    pub fn for_device(bench_ms: f32, memory_gb: Option<f32>, software: bool) -> QualityTier {
        if software {
            return Self::Low;
        }
        let by_speed = if bench_ms <= 6.0 {
            Self::Flagship
        } else if bench_ms <= 12.0 {
            Self::Mid
        } else {
            Self::Low
        };
        let by_memory = match memory_gb {
            Some(m) if m < 5.0 => Self::Low,
            Some(m) if m < 7.0 => Self::Mid,
            _ => Self::Flagship,
        };
        by_speed.min(by_memory)
    }
}

/// The tier the app runs at (set by windows and the player; tests run
/// without it, at the scene's own numbers).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quality(pub QualityTier);

/// A scene's weather.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WeatherDef {
    /// How hard it rains, 0 (dry) to 1 (downpour).
    #[serde(default)]
    pub rain: f32,
    /// How wet the road is, 0 to 1; defaults to `rain`.
    #[serde(default)]
    pub wetness: Option<f32>,
    /// Fog, 0 (none) to 1 (thick: about 60 m visibility; 0.5 is about 300 m).
    #[serde(default)]
    pub fog: f32,
}

/// The weather now (`world.set` on "Weather" changes it live).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    /// 0 (dry) to 1 (downpour).
    pub rain: f32,
    /// How wet the road is, 0 to 1.
    pub wetness: f32,
    /// The scene's dry-road grip, which wetness scales.
    pub dry_friction: f32,
    /// Fog, 0 (none) to 1 (thick).
    #[serde(default)]
    pub fog: f32,
}

/// Visibility in clear air and in the thickest fog, m.
const CLEAR_M: f32 = 1500.0;
const THICK_M: f32 = 60.0;

impl Weather {
    /// How far one can see in this fog, m (the distance where contrast drops
    /// to 5 %); `None` without fog.
    pub fn visibility_m(&self) -> Option<f32> {
        let f = self.fog.clamp(0.0, 1.0);
        (f > 0.001).then(|| CLEAR_M * (THICK_M / CLEAR_M).powf(f))
    }

    /// Grip multiplier on a road this wet: 1.0 dry, 0.7 soaked (typical
    /// asphalt goes from μ ≈ 1.0 to ≈ 0.7).
    pub fn grip(&self) -> f32 {
        1.0 - 0.3 * self.wetness.clamp(0.0, 1.0)
    }
}

/// Pedestrians for a scene: they walk to random places along the sidewalks
/// and cross at zebra crossings (with the lights where there are signals).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PedestrianDef {
    /// How many people to keep walking.
    pub count: u32,
}

/// A vehicle parked in a scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParkedDef {
    /// Vehicle name (`vehicles/<name>.vehicle.ron`).
    pub vehicle: String,
    /// Centre of mass, m (x east, y north).
    pub position: (f32, f32),
    /// Heading, degrees counter-clockwise from east.
    #[serde(default)]
    pub heading_deg: f32,
}

/// AI traffic for a scene: vehicles that drive themselves around the road
/// network, picking random destinations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrafficDef {
    /// How many AI vehicles to keep on the roads.
    pub count: u32,
    /// Vehicles to choose from at random, by name (`vehicles/<name>.vehicle.ron`).
    pub vehicles: Vec<String>,
    /// No AI vehicle starts closer than this to the player, m.
    #[serde(default = "default_clear_of_player_m")]
    pub clear_of_player_m: f32,
    /// Keep traffic within this distance of the player, m: vehicles that
    /// fall further behind reappear ahead or around (for large maps). None:
    /// traffic everywhere.
    #[serde(default)]
    pub radius_m: Option<f32>,
}

fn default_clear_of_player_m() -> f32 {
    40.0
}

/// The current scene's road network, built once when the scene is applied.
#[derive(Clone, Debug)]
pub struct Roads(pub std::sync::Arc<wp_road::RoadNetwork>);

/// A loading dock. A docked trailer has its rear at `position` and faces
/// `heading_deg` (pointing out of the dock, towards the yard).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DockDef {
    pub name: String,
    /// Centre of the dock opening at the bumpers, (x east, y north), m.
    pub position: (f32, f32),
    /// Direction out of the dock, degrees counter-clockwise from east.
    pub heading_deg: f32,
    /// Width of the bay between the guide lines, m.
    #[serde(default = "dock_width")]
    pub width_m: f32,
}

fn dock_width() -> f32 {
    3.6
}

impl DockDef {
    pub fn point(&self) -> Vec2 {
        Vec2::new(self.position.0, self.position.1)
    }
    /// Unit vector out of the dock.
    pub fn outward(&self) -> Vec2 {
        Vec2::from_angle(self.heading_deg.to_radians())
    }
}

/// The current scene's docks, for drawing and drivers.
#[derive(Clone, Debug, Default)]
pub struct Docks(pub Vec<DockDef>);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroundDef {
    /// Multiplies tyre friction: 1.0 dry asphalt, ~0.7 wet, ~0.3 snow, ~0.1 ice.
    #[serde(default = "one")]
    pub friction_scale: f32,
    /// m/s².
    #[serde(default = "earth_gravity")]
    pub gravity: f32,
}

fn one() -> f32 {
    1.0
}
fn earth_gravity() -> f32 {
    9.81
}

impl Default for GroundDef {
    fn default() -> Self {
        Self {
            friction_scale: 1.0,
            gravity: 9.81,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpawnPoint {
    pub name: String,
    /// (x east, y north), metres.
    pub position: (f32, f32),
    /// Degrees counter-clockwise from east.
    #[serde(default)]
    pub heading_deg: f32,
}

impl SceneDef {
    pub fn spawn(&self, name: &str) -> Option<&SpawnPoint> {
        self.spawns.iter().find(|s| s.name == name)
    }

    pub fn dock(&self, name: &str) -> Option<&DockDef> {
        self.docks.iter().find(|d| d.name == name)
    }

    pub fn validate(file: &Loaded<SceneDef>) -> Vec<Diagnostic> {
        let s = &file.value;
        let mut out = Vec::new();
        if !(s.ground.friction_scale > 0.0 && s.ground.friction_scale <= 2.0) {
            out.push(
                file.error_at(
                    "WP-W002",
                    "friction_scale",
                    "ground friction_scale is out of range",
                )
                .expected(format!(
                    "0 < friction_scale ≤ 2, found {}",
                    s.ground.friction_scale
                )),
            );
        }
        if !(s.ground.gravity > 0.0) {
            out.push(file.error_at("WP-W003", "gravity", "gravity must be positive"));
        }
        if s.spawns.is_empty() {
            out.push(file.warning_at(
                "WP-W004",
                "spawns",
                "scene has no spawn points; nothing can start here",
            ));
        }
        if let Some(t) = &s.terrain {
            for (field, expected) in t.problems() {
                out.push(
                    file.error_at(
                        "WP-W009",
                        field,
                        format!("terrain `{field}` is out of range"),
                    )
                    .expected(expected)
                    .help("see `wp schema scene` for each terrain field's meaning and units"),
                );
            }
            let (nx, ny) = t.grid();
            if t.problems().is_empty() && nx * ny > terrain::MAX_POINTS {
                out.push(
                    file.error_at(
                        "WP-W010",
                        "terrain",
                        format!(
                            "terrain has {} grid points; at most {} fit in memory on phones",
                            nx * ny,
                            terrain::MAX_POINTS
                        ),
                    )
                    .help("raise cell_m or make size_m smaller"),
                );
            }
        }
        for w in &s.water {
            for (field, expected) in w.problems() {
                out.push(
                    file.error_at("WP-W012", field, format!("water `{field}` is out of range"))
                        .expected(expected)
                        .help("see `wp schema scene` for each water field's meaning and units"),
                );
            }
            if s.terrain.is_none() {
                out.push(
                    file.warning_at(
                        "WP-W013",
                        "water",
                        "water without terrain has nowhere to be (the plain is flat)",
                    )
                    .help(
                        "add `terrain` with hollows (hills with negative height_m) below level_m",
                    ),
                );
            }
        }
        if let Some(g) = &s.grass {
            for (field, expected) in g.problems() {
                out.push(
                    file.error_at("WP-W011", field, format!("grass `{field}` is out of range"))
                        .expected(expected)
                        .help("see `wp schema scene` for each grass field's meaning and units"),
                );
            }
        }
        for (i, sp) in s.spawns.iter().enumerate() {
            if s.spawns[..i].iter().any(|o| o.name == sp.name) {
                out.push(
                    file.error_at(
                        "WP-W001",
                        "spawns",
                        format!("spawn point `{}` is defined twice", sp.name),
                    )
                    .help("give every spawn point a unique name"),
                );
            }
        }
        out
    }
}

/// A human-readable name, shown by the inspector and control protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Name(pub String);

impl Component for Name {
    const NAME: &'static str = "Name";
}

/// Marks the vehicle the player (or a test driver) controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Player;

impl Component for Player {
    const NAME: &'static str = "Player";
}

/// Apply a scene's ground settings to the world.
pub fn apply_scene(
    world: &mut World,
    scene: &SceneDef,
    roads: Option<&wp_road::RoadNetworkDef>,
    scenery: Option<&scenery::SceneryDef>,
) {
    let net = roads.map(|def| std::sync::Arc::new(wp_road::RoadNetwork::build(def)));
    let land = scene.terrain.as_ref().map(|def| {
        let field = terrain::build(def, net.as_deref(), &scene.water);
        let (owner, levels) = if scene.water.is_empty() {
            (Vec::new(), None)
        } else {
            let (o, l) = terrain::flood(&field, &scene.water);
            (o, Some(std::sync::Arc::new(l)))
        };
        std::sync::Arc::new(terrain::BuiltTerrain {
            def: def.clone(),
            field: std::sync::Arc::new(field),
            water: scene.water.clone(),
            water_owner: owner,
            water_levels: levels,
        })
    });
    match land.as_ref().and_then(|t| t.water_levels.clone()) {
        Some(levels) => world.insert_resource(wp_physics::Water(levels)),
        None => {
            world.remove_resource::<wp_physics::Water>();
        }
    }
    match &land {
        Some(t) => {
            world.insert_resource(wp_physics::Terrain(t.field.clone()));
            world.insert_resource(terrain::CurrentTerrain(t.clone()));
        }
        None => {
            world.remove_resource::<wp_physics::Terrain>();
            world.remove_resource::<terrain::CurrentTerrain>();
        }
    }
    match scenery {
        Some(def) => {
            let mut built = scenery::Scenery::build(def, net.as_deref());
            if let Some(t) = &land {
                built.settle(&t.field);
            }
            // Buildings are solid.
            let mut walls =
                wp_physics::Walls::new(built.buildings.iter().map(|b| b.footprint.as_slice()));
            let trunks: Vec<[glam::Vec2; 4]> = built.trees.iter().map(|t| t.trunk()).collect();
            walls.add(trunks.iter().map(|t| t.as_slice()), "tree");
            // Solid props (only those with a size, so plain scenes stay as they were).
            let solid: Vec<[glam::Vec2; 4]> =
                built.props.iter().filter_map(|p| p.outline()).collect();
            if !solid.is_empty() {
                walls.add(solid.iter().map(|o| o.as_slice()), "prop");
            }
            world.insert_resource(walls);
            world.insert_resource(scenery::CurrentScenery(std::sync::Arc::new(built)));
        }
        None => {
            world.remove_resource::<scenery::CurrentScenery>();
            world.remove_resource::<wp_physics::Walls>();
        }
    }
    let built_scenery = world
        .get_resource::<scenery::CurrentScenery>()
        .filter(|_| scenery.is_some())
        .map(|s| s.0.clone());
    match &scene.grass {
        Some(def) => {
            let map = grass::build(
                def,
                net.as_deref(),
                built_scenery.as_deref(),
                land.as_ref().map(|t| t.field.as_ref()),
                land.as_ref().and_then(|t| t.water_levels.as_deref()),
            );
            world.insert_resource(grass::CurrentGrass(std::sync::Arc::new(
                grass::BuiltGrass {
                    def: def.clone(),
                    map,
                },
            )));
        }
        None => {
            world.remove_resource::<grass::CurrentGrass>();
        }
    }
    match net {
        Some(net) => world.insert_resource(Roads(net)),
        None => {
            world.remove_resource::<Roads>();
        }
    }
    let weather = scene.weather.clone().unwrap_or_default();
    let w = Weather {
        rain: weather.rain.clamp(0.0, 1.0),
        wetness: weather.wetness.unwrap_or(weather.rain).clamp(0.0, 1.0),
        dry_friction: scene.ground.friction_scale,
        fog: weather.fog.clamp(0.0, 1.0),
    };
    world.insert_resource(Ground {
        friction_scale: scene.ground.friction_scale * w.grip(),
    });
    world.insert_reflected_resource("Weather", w);
    if let Some(t) = &scene.time_of_day {
        world.insert_reflected_resource(
            "TimeOfDay",
            TimeOfDay {
                hour: t.hour.rem_euclid(24.0),
                rate: t.hours_per_hour.max(0.0),
            },
        );
    }
    world.insert_resource(Gravity(scene.ground.gravity));
    world.insert_resource(Docks(scene.docks.clone()));
}

/// Spawn position and heading (radians) of a spawn point.
pub fn spawn_transform(sp: &SpawnPoint) -> (Vec2, f32) {
    (
        Vec2::new(sp.position.0, sp.position.1),
        sp.heading_deg.to_radians(),
    )
}

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn name(&self) -> &'static str {
        "world"
    }
    fn dependencies(&self) -> Vec<&'static str> {
        vec!["physics"]
    }
    fn build(&self, app: &mut App) {
        app.world.register::<Name>();
        app.world.register::<Player>();
        // Wet roads: grip follows the weather (which may change live).
        app.add_system(wp_ecs::Stage::Input, "weather_grip", |w| {
            if let Some(weather) = w.get_resource::<Weather>().map(|x| *x)
                && let Some(mut g) = w.get_resource_mut::<Ground>()
            {
                g.friction_scale = weather.dry_friction * weather.grip();
            }
        })
        .reads_resource("Weather")
        .writes_resource("Ground");
        app.add_system(wp_ecs::Stage::Input, "clock", |w| {
            let dt = w.resource::<wp_core::SimTime>().dt();
            if let Some(mut t) = w.get_resource_mut::<TimeOfDay>()
                && t.rate > 0.0
            {
                t.hour = (t.hour + t.rate * dt / 3600.0).rem_euclid(24.0);
            }
        })
        .writes_resource("TimeOfDay");
    }
}

#[cfg(test)]
mod tier_tests {
    use super::QualityTier::{self, *};

    #[test]
    fn tiers_follow_speed_and_memory() {
        assert_eq!(QualityTier::for_device(3.0, Some(12.0), false), Flagship);
        assert_eq!(QualityTier::for_device(9.0, Some(12.0), false), Mid);
        assert_eq!(QualityTier::for_device(20.0, None, false), Low);
        // A fast GPU in a 6 GB phone stays Mid; in a 4 GB phone, Low.
        assert_eq!(QualityTier::for_device(3.0, Some(5.6), false), Mid);
        assert_eq!(QualityTier::for_device(3.0, Some(3.7), false), Low);
        // No GPU: a CPU rasteriser is always Low.
        assert_eq!(QualityTier::for_device(1.0, None, true), Low);
        assert!(Low < Mid && Mid < Flagship);
    }
}
