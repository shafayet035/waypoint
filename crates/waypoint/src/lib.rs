//! Waypoint: an agent-first driving engine.
//!
//! This facade crate re-exports the engine crates, provides [`DefaultPlugins`]
//! and loads projects ([`project::Project`]): a folder with `game.toml`,
//! `vehicles/*.vehicle.ron`, `scenes/*.scene.ron` and `tests/*.scenario.ron`.

pub mod project;

pub use wp_app as app;
pub use wp_core as core;
pub use wp_data as data;
pub use wp_ecs as ecs;
pub use wp_physics as physics;
pub use wp_test as test;
pub use wp_vehicle as vehicle;
pub use wp_world as world;

use wp_app::{App, Plugin, PluginGroup};

/// Every engine plugin. A game can disable any of them by name in `game.toml`.
pub struct DefaultPlugins;

impl PluginGroup for DefaultPlugins {
    fn plugins(self) -> Vec<Box<dyn Plugin>> {
        vec![
            Box::new(wp_physics::PhysicsPlugin),
            Box::new(wp_world::WorldPlugin),
            Box::new(wp_vehicle::VehiclePlugin),
        ]
    }
}

/// An unfinished app with `DefaultPlugins`, minus the disabled ones.
pub fn make_app(disabled: &[String]) -> App {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins);
    for d in disabled {
        app.disable_plugin(d);
    }
    app
}

pub mod prelude {
    pub use crate::DefaultPlugins;
    pub use wp_app::{App, Plugin, PluginGroup};
    pub use wp_core::glam::{Vec2, vec2};
    pub use wp_core::{Diagnostic, SimRng, SimTime};
    pub use wp_ecs::{Component, Entity, Events, Stage, World};
    pub use wp_physics::{Gravity, Ground, Pose, RigidBody, Velocity};
    pub use wp_vehicle::{Vehicle, VehicleDef, VehicleInput, spawn_vehicle, telemetry};
    pub use wp_world::{Name, Player, SceneDef};
}
