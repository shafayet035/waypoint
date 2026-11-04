//! Scenes. M0 scenes are a flat ground plane with named spawn points; roads,
//! tiles and streaming arrive with `wp_road` and world streaming in M2–M3.

use glam::Vec2;
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
    /// AI traffic on the road network.
    #[serde(default)]
    pub traffic: Option<TrafficDef>,
    /// Vehicles standing still in the scene, brakes on (obstacles, set dressing).
    #[serde(default)]
    pub parked: Vec<ParkedDef>,
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
pub fn apply_scene(world: &mut World, scene: &SceneDef, roads: Option<&wp_road::RoadNetworkDef>) {
    match roads {
        Some(def) => {
            world.insert_resource(Roads(std::sync::Arc::new(wp_road::RoadNetwork::build(def))))
        }
        None => {
            world.remove_resource::<Roads>();
        }
    }
    world.insert_resource(Ground {
        friction_scale: scene.ground.friction_scale,
    });
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
    }
}
