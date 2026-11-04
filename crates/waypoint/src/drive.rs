//! Putting a vehicle on the road. Shared by every front end (CLI, MCP server,
//! player) so they build sessions the same way.

use crate::project::Project;
use wp_app::App;
use wp_core::{Diagnostic, SimRng};
use wp_ecs::Entity;
use wp_vehicle::VehicleInput;
use wp_world::{Name, Player};

/// What to drive: a scenario (its scene, vehicle, trailer, spawn and driver),
/// or a vehicle at a spawn point with constant inputs.
#[derive(Clone, Debug, Default)]
pub struct DriveSpec {
    pub scenario: Option<String>,
    /// Scene path; defaults to `game.toml`'s `start_scene`.
    pub scene: Option<String>,
    /// Spawn point; defaults to the scene's first.
    pub spawn: Option<String>,
    /// Vehicle name; defaults to the first vehicle (alphabetical).
    pub vehicle: Option<String>,
    /// Semi-trailer to couple to the vehicle's fifth wheel.
    pub trailer: Option<String>,
    /// Constant driver controls (ignored for scenarios, which have their own driver).
    pub input: VehicleInput,
    pub seed: u64,
}

/// A finished app with the player spawned, ready to tick.
pub struct Drive {
    pub app: App,
    pub player: Entity,
    /// For messages: "scenario `x`" or "`vehicle` in scene".
    pub label: String,
}

fn unknown<'a>(what: &str, name: &str, known: impl Iterator<Item = &'a String>) -> Diagnostic {
    let list: Vec<&str> = known.map(String::as_str).collect();
    Diagnostic::error("WP-C001", format!("unknown {what} `{name}`")).help(format!(
        "known: {}",
        if list.is_empty() {
            "(none)".into()
        } else {
            list.join(", ")
        }
    ))
}

impl DriveSpec {
    pub fn scenario(name: &str) -> Self {
        Self {
            scenario: Some(name.to_string()),
            ..Default::default()
        }
    }

    pub fn build(&self, project: &Project) -> Result<Drive, Diagnostic> {
        let make_app = || crate::make_app(project.disabled_plugins());
        if let Some(name) = &self.scenario {
            if !project.scenarios.contains_key(name) {
                return Err(unknown("scenario", name, project.scenarios.keys()));
            }
            let input = project.scenario_input(name).ok_or_else(|| {
                Diagnostic::error("WP-C001", format!("scenario `{name}` is incomplete"))
                    .help("run `wp check`")
            })?;
            let (app, player) = wp_test::prepare(&input, &make_app)?;
            return Ok(Drive {
                app,
                player,
                label: format!("scenario `{name}`"),
            });
        }

        let scene_path = self
            .scene
            .clone()
            .unwrap_or_else(|| project.game.value.start_scene.clone());
        let scene = &project
            .scenes
            .get(&scene_path)
            .ok_or_else(|| unknown("scene", &scene_path, project.scenes.keys()))?
            .value;
        let vehicle = match &self.vehicle {
            Some(v) => project
                .vehicles
                .get(v)
                .ok_or_else(|| unknown("vehicle", v, project.vehicles.keys()))?,
            None => project.vehicles.values().next().ok_or_else(|| {
                Diagnostic::error("WP-G011", "the project has no vehicles")
                    .help("add a vehicles/<name>.vehicle.ron file")
            })?,
        };
        let spawn = match &self.spawn {
            Some(s) => scene
                .spawn(s)
                .ok_or_else(|| unknown("spawn point", s, scene.spawns.iter().map(|p| &p.name)))?,
            None => scene.spawns.first().ok_or_else(|| {
                Diagnostic::error(
                    "WP-W004",
                    format!("scene `{}` has no spawn points", scene.name),
                )
                .help("add an entry to `spawns`")
            })?,
        };

        let mut app = make_app();
        app.finish()?;
        app.world
            .insert_reflected_resource("SimRng", SimRng::new(self.seed));
        wp_world::apply_scene(&mut app.world, scene, project.scene_roads(scene));
        if let (Some(t), Some(vehicles)) = (&scene.traffic, project.scene_traffic(scene)) {
            wp_traffic::setup(&mut app.world, t, &vehicles);
        }
        let (pos, heading) = wp_world::spawn_transform(spawn);
        let player = wp_vehicle::spawn_vehicle(&mut app.world, &vehicle.value, pos, heading);
        app.world.insert(player, Name(vehicle.value.name.clone()));
        app.world.insert(player, Player);
        *app.world
            .get_mut::<VehicleInput>(player)
            .expect("spawned vehicles have inputs") = VehicleInput {
            throttle: self.input.throttle.clamp(0.0, 1.0),
            brake: self.input.brake.clamp(0.0, 1.0),
            steer: self.input.steer.clamp(-1.0, 1.0),
            reverse: self.input.reverse,
        };
        if let Some(tname) = &self.trailer {
            let tdef = &project
                .trailers
                .get(tname)
                .ok_or_else(|| unknown("trailer", tname, project.trailers.keys()))?
                .value;
            let t = wp_vehicle::spawn_trailer(&mut app.world, tdef, player).ok_or_else(|| {
                Diagnostic::error(
                    "WP-S019",
                    format!(
                        "vehicle `{}` has no fifth wheel for a trailer",
                        vehicle.value.name
                    ),
                )
                .help("pick a tractor unit")
            })?;
            app.world.insert(t, Name(tdef.name.clone()));
        }
        Ok(Drive {
            app,
            player,
            label: format!("`{}` in {scene_path}", vehicle.value.name),
        })
    }
}
