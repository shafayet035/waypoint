//! WebAssembly mod modules.
//!
//! A mod can ship compiled code (`.wasm`, or `.wat` text) that runs every
//! tick. Modules run in an interpreter (wasmi: no JIT, so the same code runs
//! on iOS and Android), with a fuel budget per tick, and can only do what
//! their mod's capabilities allow. Host functions live in the `waypoint`
//! import module:
//!
//! | Function | Capability |
//! | --- | --- |
//! | `time_s() -> f64` | none |
//! | `log(ptr: i32, len: i32)` (UTF-8 in the module's exported `memory`) | `log` |
//! | `vehicle_count() -> i32`, `player() -> i32` (index or −1) | `read_vehicles` |
//! | `vehicle_x(i) / vehicle_y(i) / vehicle_heading(i) / vehicle_speed_kmh(i) -> f32` | `read_vehicles` |
//! | `input_throttle(i) / input_brake(i) / input_steer(i) -> f32` | `read_vehicles` |
//! | `set_input(i, throttle: f32, brake: f32, steer: f32)` | `drive_vehicles` |
//!
//! A module exports `on_tick(dt: f32)`. Vehicles are listed in entity order
//! each tick. A trap (a bug, running out of fuel, or a host call without
//! its capability) switches the module off and records why in the
//! reflected `WasmMods` resource.

use serde::{Deserialize, Serialize};
use wasmi::{Caller, Config, Engine, Linker, Module, Store, TypedFunc};
use wp_app::{App, Plugin};
use wp_ecs::{Entity, Stage, World};
use wp_physics::{Pose, Velocity};
use wp_vehicle::{Vehicle, VehicleInput};

/// Instructions a module may run per tick (about a millisecond's worth).
pub const FUEL_PER_TICK: u64 = 2_000_000;

/// What a mod's code may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Log,
    ReadVehicles,
    DriveVehicles,
}

impl Capability {
    pub const ALL: [(&'static str, Capability); 3] = [
        ("log", Capability::Log),
        ("read_vehicles", Capability::ReadVehicles),
        ("drive_vehicles", Capability::DriveVehicles),
    ];

    pub fn parse(s: &str) -> Option<Capability> {
        Self::ALL.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
    }

    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, c)| *c == self)
            .map_or("?", |(n, _)| n)
    }
}

/// A module ready to run: compiled WebAssembly and what it may do.
#[derive(Clone, Debug)]
pub struct ModuleDef {
    /// For messages: "mod_id/path.wasm".
    pub name: String,
    pub wasm: Vec<u8>,
    pub capabilities: Vec<Capability>,
}

/// Turn `.wat` text or `.wasm` bytes into WebAssembly and check it compiles
/// and exports `on_tick(f32)`.
pub fn compile(source: &[u8], text: bool) -> Result<Vec<u8>, String> {
    let wasm = if text {
        let s = std::str::from_utf8(source).map_err(|e| e.to_string())?;
        wat::parse_str(s).map_err(|e| e.to_string())?
    } else {
        source.to_vec()
    };
    let engine = Engine::default();
    let module = Module::new(&engine, &wasm[..]).map_err(|e| e.to_string())?;
    let has_tick = module
        .exports()
        .any(|e| e.name() == "on_tick" && e.ty().func().is_some());
    if !has_tick {
        return Err("the module does not export `on_tick(dt: f32)`".into());
    }
    Ok(wasm)
}

/// A vehicle as modules see it this tick.
#[derive(Clone, Copy, Debug, Default)]
struct VehicleView {
    entity: Option<Entity>,
    x: f32,
    y: f32,
    heading: f32,
    speed_kmh: f32,
    input: VehicleInput,
}

/// Host state shared with one module's calls.
struct Host {
    caps: Vec<Capability>,
    mod_name: String,
    time_s: f64,
    vehicles: Vec<VehicleView>,
    player: i32,
    set: Vec<(usize, VehicleInput)>,
    log: Vec<String>,
}

impl Host {
    fn need(&self, c: Capability) -> Result<(), wasmi::Error> {
        if self.caps.contains(&c) {
            Ok(())
        } else {
            Err(wasmi::Error::new(format!(
                "capability `{}` is not granted to {} (add it to the mod's capabilities)",
                c.name(),
                self.mod_name
            )))
        }
    }

    fn vehicle(&self, i: i32) -> VehicleView {
        self.vehicles
            .get(i.max(0) as usize)
            .copied()
            .unwrap_or_default()
    }
}

struct Running {
    store: Store<Host>,
    on_tick: TypedFunc<f32, ()>,
}

/// One module's state, as agents see it (`world.resources` → `WasmMods`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModuleStatus {
    pub name: String,
    pub capabilities: Vec<Capability>,
    pub running: bool,
    /// Why it stopped (a trap, no fuel, a missing capability).
    pub error: Option<String>,
    /// The last log lines.
    pub log: Vec<String>,
}

/// Reflected resource: every module and what it is doing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WasmMods(pub Vec<ModuleStatus>);

/// The live modules (not reflected: interpreter state).
#[derive(Default)]
pub struct WasmRuntime {
    modules: Vec<Option<Running>>,
}

/// Lines kept per module's log.
const LOG_LINES: usize = 20;

/// Instantiate `defs` into the world. Modules that fail to start are listed
/// with their error and do not run.
pub fn load(world: &mut World, defs: &[ModuleDef]) {
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let mut runtime = WasmRuntime::default();
    let mut status = WasmMods::default();
    for def in defs {
        let started = start(&engine, def);
        status.0.push(ModuleStatus {
            name: def.name.clone(),
            capabilities: def.capabilities.clone(),
            running: started.is_ok(),
            error: started.as_ref().err().cloned(),
            log: Vec::new(),
        });
        runtime.modules.push(started.ok());
    }
    world.insert_resource(runtime);
    world.insert_reflected_resource("WasmMods", status);
}

fn start(engine: &Engine, def: &ModuleDef) -> Result<Running, String> {
    let module = Module::new(engine, &def.wasm[..]).map_err(|e| e.to_string())?;
    let host = Host {
        caps: def.capabilities.clone(),
        mod_name: def.name.clone(),
        time_s: 0.0,
        vehicles: Vec::new(),
        player: -1,
        set: Vec::new(),
        log: Vec::new(),
    };
    let mut store = Store::new(engine, host);
    let mut linker = <Linker<Host>>::new(engine);
    let err = |e: wasmi::errors::LinkerError| e.to_string();
    linker
        .func_wrap("waypoint", "time_s", |c: Caller<'_, Host>| c.data().time_s)
        .map_err(err)?;
    linker
        .func_wrap(
            "waypoint",
            "log",
            |mut c: Caller<'_, Host>, ptr: i32, len: i32| -> Result<(), wasmi::Error> {
                c.data().need(Capability::Log)?;
                let mem = c
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| wasmi::Error::new("log needs an exported `memory`"))?;
                let mut buf = vec![0u8; len.clamp(0, 1024) as usize];
                mem.read(&c, ptr.max(0) as usize, &mut buf)
                    .map_err(|e| wasmi::Error::new(e.to_string()))?;
                let line = String::from_utf8_lossy(&buf).to_string();
                c.data_mut().log.push(line);
                Ok(())
            },
        )
        .map_err(err)?;
    linker
        .func_wrap(
            "waypoint",
            "vehicle_count",
            |c: Caller<'_, Host>| -> Result<i32, wasmi::Error> {
                c.data().need(Capability::ReadVehicles)?;
                Ok(c.data().vehicles.len() as i32)
            },
        )
        .map_err(err)?;
    linker
        .func_wrap(
            "waypoint",
            "player",
            |c: Caller<'_, Host>| -> Result<i32, wasmi::Error> {
                c.data().need(Capability::ReadVehicles)?;
                Ok(c.data().player)
            },
        )
        .map_err(err)?;
    type Getter = fn(&VehicleView) -> f32;
    let getters: [(&str, Getter); 7] = [
        ("vehicle_x", |v| v.x),
        ("vehicle_y", |v| v.y),
        ("vehicle_heading", |v| v.heading),
        ("vehicle_speed_kmh", |v| v.speed_kmh),
        ("input_throttle", |v| v.input.throttle),
        ("input_brake", |v| v.input.brake),
        ("input_steer", |v| v.input.steer),
    ];
    for (name, get) in getters {
        linker
            .func_wrap(
                "waypoint",
                name,
                move |c: Caller<'_, Host>, i: i32| -> Result<f32, wasmi::Error> {
                    c.data().need(Capability::ReadVehicles)?;
                    Ok(get(&c.data().vehicle(i)))
                },
            )
            .map_err(err)?;
    }
    linker
        .func_wrap(
            "waypoint",
            "set_input",
            |mut c: Caller<'_, Host>,
             i: i32,
             throttle: f32,
             brake: f32,
             steer: f32|
             -> Result<(), wasmi::Error> {
                c.data().need(Capability::DriveVehicles)?;
                let n = c.data().vehicles.len();
                if i < 0 || i as usize >= n {
                    return Err(wasmi::Error::new(format!("no vehicle {i} (there are {n})")));
                }
                let reverse = c.data().vehicles[i as usize].input.reverse;
                let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
                c.data_mut().set.push((
                    i as usize,
                    VehicleInput {
                        throttle: finite(throttle).clamp(0.0, 1.0),
                        brake: finite(brake).clamp(0.0, 1.0),
                        steer: finite(steer).clamp(-1.0, 1.0),
                        reverse,
                    },
                ));
                Ok(())
            },
        )
        .map_err(err)?;
    store.set_fuel(FUEL_PER_TICK).map_err(|e| e.to_string())?;
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| e.to_string())?;
    let on_tick = instance
        .get_typed_func::<f32, ()>(&store, "on_tick")
        .map_err(|e| format!("`on_tick(dt: f32)`: {e}"))?;
    Ok(Running { store, on_tick })
}

/// Run every module's `on_tick`, then apply what they asked for.
pub fn run_modules(world: &mut World) {
    let Some(mut runtime) = world.remove_resource::<WasmRuntime>() else {
        return;
    };
    let time = world.resource::<wp_core::SimTime>().clone();
    let dt = time.dt();
    let player = world
        .query::<&wp_world::Player>()
        .entities()
        .first()
        .copied();
    let mut vehicles: Vec<VehicleView> = Vec::new();
    world
        .query::<(&Pose, &Velocity, &Vehicle, &VehicleInput)>()
        .for_each(|e, (pose, vel, _, input)| {
            vehicles.push(VehicleView {
                entity: Some(e),
                x: pose.position.x,
                y: pose.position.y,
                heading: pose.heading,
                speed_kmh: vel.linear.length() * 3.6,
                input: *input,
            });
        });
    vehicles.sort_by_key(|v| v.entity);
    let player_index = vehicles
        .iter()
        .position(|v| v.entity == player && player.is_some())
        .map_or(-1, |i| i as i32);
    let mut results = Vec::new();
    for (k, slot) in runtime.modules.iter_mut().enumerate() {
        let Some(running) = slot else {
            continue;
        };
        {
            let host = running.store.data_mut();
            host.time_s = time.seconds();
            host.vehicles = vehicles.clone();
            host.player = player_index;
            host.set.clear();
        }
        let outcome = running
            .store
            .set_fuel(FUEL_PER_TICK)
            .map_err(|e| e.to_string())
            .and_then(|_| {
                running
                    .on_tick
                    .call(&mut running.store, dt)
                    .map_err(|e| e.to_string())
            });
        let host = running.store.data_mut();
        let set = std::mem::take(&mut host.set);
        let log = std::mem::take(&mut host.log);
        match outcome {
            Ok(()) => {
                // Later modules see earlier modules' inputs next tick.
                for (i, input) in &set {
                    vehicles[*i].input = *input;
                }
                results.push((k, set, log, None));
            }
            Err(e) => {
                results.push((k, Vec::new(), log, Some(e)));
                *slot = None;
            }
        }
    }
    for (k, set, log, error) in results {
        for (i, input) in set {
            if let Some(e) = vehicles[i].entity
                && let Some(mut v) = world.get_mut::<VehicleInput>(e)
            {
                *v = input;
            }
        }
        if let Some(mut status) = world.get_resource_mut::<WasmMods>()
            && let Some(s) = status.0.get_mut(k)
        {
            s.log.extend(log);
            let excess = s.log.len().saturating_sub(LOG_LINES);
            s.log.drain(..excess);
            if let Some(e) = error {
                s.running = false;
                s.error = Some(e);
            }
        }
    }
    world.insert_resource(runtime);
}

/// Runs mod modules each tick, after the AI drivers (Agents stage).
pub struct WasmPlugin;

impl Plugin for WasmPlugin {
    fn name(&self) -> &'static str {
        "wasm"
    }
    fn dependencies(&self) -> Vec<&'static str> {
        vec!["vehicle"]
    }
    fn build(&self, app: &mut App) {
        app.add_system(Stage::Agents, "wasm_mods", run_modules)
            .writes::<VehicleInput>()
            .reads_resource("WasmRuntime");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_car() -> (App, Entity) {
        let def: wp_vehicle::VehicleDef = wp_data::parse_ron(
            "hatchback.vehicle.ron",
            include_str!("../../../examples/sandbox/vehicles/hatchback.vehicle.ron"),
        )
        .unwrap();
        let mut app = App::new();
        app.add_plugin(wp_physics::PhysicsPlugin)
            .add_plugin(wp_vehicle::VehiclePlugin)
            .add_plugin(WasmPlugin);
        app.finish().unwrap();
        let e = wp_vehicle::spawn_vehicle(&mut app.world, &def, wp_core::glam::Vec2::ZERO, 0.0);
        app.world.insert(e, wp_world::Player);
        (app, e)
    }

    /// Full throttle until 30 km/h, then coast: a cruise limiter.
    const LIMITER: &str = r#"
      (module
        (import "waypoint" "player" (func $player (result i32)))
        (import "waypoint" "vehicle_speed_kmh" (func $speed (param i32) (result f32)))
        (import "waypoint" "set_input" (func $set (param i32 f32 f32 f32)))
        (import "waypoint" "log" (func $log (param i32 i32)))
        (memory (export "memory") 1)
        (data (i32.const 0) "limiter on")
        (global $said (mut i32) (i32.const 0))
        (func (export "on_tick") (param $dt f32)
          (local $p i32)
          (if (i32.eqz (global.get $said))
            (then (call $log (i32.const 0) (i32.const 10)) (global.set $said (i32.const 1))))
          (local.set $p (call $player))
          (if (i32.ge_s (local.get $p) (i32.const 0))
            (then
              (if (f32.lt (call $speed (local.get $p)) (f32.const 30))
                (then (call $set (local.get $p) (f32.const 1) (f32.const 0) (f32.const 0)))
                (else (call $set (local.get $p) (f32.const 0) (f32.const 0) (f32.const 0))))))))
    "#;

    fn module(src: &str, caps: &[Capability]) -> ModuleDef {
        ModuleDef {
            name: "test/limiter.wat".into(),
            wasm: compile(src.as_bytes(), true).unwrap(),
            capabilities: caps.to_vec(),
        }
    }

    #[test]
    fn a_module_drives_the_player_within_its_capabilities() {
        let (mut app, e) = world_with_car();
        load(
            &mut app.world,
            &[module(
                LIMITER,
                &[
                    Capability::Log,
                    Capability::ReadVehicles,
                    Capability::DriveVehicles,
                ],
            )],
        );
        app.run_ticks(60 * 20);
        let kmh = wp_vehicle::telemetry(&app.world, e).unwrap().speed_kmh;
        assert!((26.0..34.0).contains(&kmh), "held near 30 km/h: {kmh:.1}");
        let status = app.world.resource::<WasmMods>();
        assert!(status.0[0].running, "{:?}", status.0[0]);
        assert_eq!(status.0[0].log, vec!["limiter on".to_string()]);
    }

    #[test]
    fn missing_capabilities_and_runaway_loops_stop_a_module() {
        let (mut app, e) = world_with_car();
        let spin = r#"(module (func (export "on_tick") (param f32) (loop (br 0))))"#;
        load(
            &mut app.world,
            &[
                module(LIMITER, &[Capability::ReadVehicles, Capability::Log]),
                module(spin, &[]),
            ],
        );
        app.run_ticks(10);
        let status = app.world.resource::<WasmMods>().clone();
        assert!(!status.0[0].running);
        assert!(
            status.0[0]
                .error
                .as_deref()
                .unwrap()
                .contains("drive_vehicles"),
            "{:?}",
            status.0[0]
        );
        assert!(!status.0[1].running, "fuel stops an endless loop");
        // The car was never driven.
        assert_eq!(app.world.get::<VehicleInput>(e).unwrap().throttle, 0.0);
    }

    #[test]
    fn compile_rejects_modules_without_on_tick() {
        assert!(compile(b"(module)", true).unwrap_err().contains("on_tick"));
        assert!(compile(b"(module (func", true).is_err());
        assert_eq!(
            Capability::parse("drive_vehicles"),
            Some(Capability::DriveVehicles)
        );
    }
}
