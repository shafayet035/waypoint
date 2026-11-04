//! Scenes. M0 scenes are a flat ground plane with named spawn points; roads,
//! tiles and streaming arrive with `wp_road` and world streaming in M2–M3.

mod look;
pub mod scenery;
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
    /// Rain. Wet roads have less grip; drivers slow down.
    #[serde(default)]
    pub weather: Option<WeatherDef>,
    /// Time of day (sun, sky, headlights, street lamps). Absent: early afternoon.
    #[serde(default)]
    pub time_of_day: Option<TimeOfDayDef>,
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
}

impl Weather {
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
    match scenery {
        Some(def) => world.insert_resource(scenery::CurrentScenery(std::sync::Arc::new(
            scenery::Scenery::build(def, net.as_deref()),
        ))),
        None => {
            world.remove_resource::<scenery::CurrentScenery>();
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
