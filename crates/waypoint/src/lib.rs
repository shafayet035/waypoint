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

/// Start a project for play: a scenario's setup (with its driver) if `scenario`
/// is given, else the first vehicle at the first spawn of `start_scene`.
/// Returns the finished app and the player entity.
pub fn start(
    project: &project::Project,
    scenario: Option<&str>,
    seed: u64,
) -> Result<(App, wp_ecs::Entity), wp_core::Diagnostic> {
    use wp_core::Diagnostic;
    let make_app = || make_app(project.disabled_plugins());
    if let Some(name) = scenario {
        let sc = project.scenarios.get(name).ok_or_else(|| {
            Diagnostic::error("WP-C001", format!("unknown scenario `{name}`")).help(format!(
                "scenarios: {}",
                project
                    .scenarios
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let s = &sc.value;
        let input = wp_test::ScenarioInput {
            name,
            scenario: s,
            scene: &project.scenes[&s.scene].value,
            vehicle: &project.vehicles[&s.player.vehicle].value,
        };
        return wp_test::prepare(&input, &make_app);
    }
    let scene = &project.scenes[&project.game.value.start_scene].value;
    let vehicle = project.vehicles.values().next().ok_or_else(|| {
        Diagnostic::error("WP-G011", "the project has no vehicles")
            .help("add a vehicles/<name>.vehicle.ron file")
    })?;
    let spawn = scene.spawns.first().ok_or_else(|| {
        Diagnostic::error(
            "WP-W004",
            format!("scene `{}` has no spawn points", scene.name),
        )
        .help("add an entry to `spawns`")
    })?;
    let mut app = make_app();
    app.finish()?;
    app.world
        .insert_reflected_resource("SimRng", wp_core::SimRng::new(seed));
    wp_world::apply_scene(&mut app.world, scene);
    let (pos, heading) = wp_world::spawn_transform(spawn);
    let player = wp_vehicle::spawn_vehicle(&mut app.world, &vehicle.value, pos, heading);
    app.world
        .insert(player, wp_world::Name(vehicle.value.name.clone()));
    app.world.insert(player, wp_world::Player);
    Ok((app, player))
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
