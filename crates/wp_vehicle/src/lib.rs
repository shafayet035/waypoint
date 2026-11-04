//! Vehicles: the `.vehicle.ron` format and the vehicle dynamics system.
//!
//! A vehicle entity has [`Pose`], [`Velocity`] and [`RigidBody`] from
//! `wp_physics`, plus [`Vehicle`] (part state) and [`VehicleInput`] (driver
//! controls). Each physics substep, [`dynamics::vehicle_dynamics`] runs body
//! roll and pitch, wheel loads, steering, drivetrain, brakes (hydraulic or air,
//! with optional ABS), tyre slip and forces, and adds the resulting force and
//! yaw moment to the rigid body. Notable moments are sent as [`VehicleEvent`]s.

pub mod assist;
pub mod collision;
pub mod def;
pub mod dynamics;
pub mod parts;
pub mod trailer;

use glam::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wp_app::{App, Plugin};
use wp_ecs::{Component, Entity, Stage, World};
use wp_physics::{Gravity, Pose, RigidBody, Velocity};

pub use assist::{Controls, DriverAids, steer_limit};
pub use collision::{Contacts, Footprint, detect_collisions, vehicle_footprint};
pub use def::{AirBrakeDef, BodyDef, BodyStyle, Drive, HitchDef, SuspensionDef, VehicleDef};
pub use dynamics::vehicle_dynamics;
pub use parts::AirState;
pub use trailer::{
    Trailer, TrailerDef, TrailerLibrary, TrailerPlugin, spawn_trailer, trailer_dynamics,
};

/// Wheel order used everywhere: front-left, front-right, rear-left, rear-right.
pub const WHEEL_NAMES: [&str; 4] = ["FL", "FR", "RL", "RR"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WheelState {
    /// Spin rate, rad/s.
    pub omega: f32,
    /// Rotation angle, rad, in [0, 2π). Used by the renderer.
    pub angle: f32,
    /// Relaxed longitudinal slip κ.
    pub slip_ratio: f32,
    /// Relaxed lateral slip tan α.
    pub slip_tan: f32,
    /// Vertical load, N.
    pub load: f32,
    /// Tyre force in the wheel frame (x forward, y left), N.
    pub force: Vec2,
    /// ABS valve opening: 1 = full brake pressure, 0 = released.
    #[serde(default = "one")]
    pub abs_factor: f32,
    /// True while the wheel carries no load (lifted off the ground).
    #[serde(default)]
    pub lifted: bool,
}

fn one() -> f32 {
    1.0
}

/// Runtime state of a vehicle's parts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vehicle {
    /// Name of the [`VehicleDef`] in the [`VehicleLibrary`].
    pub model: String,
    pub wheels: [WheelState; 4],
    /// Front-wheel angle, rad, positive to the left.
    pub steer_angle: f32,
    /// Current forward gear, starting at 1.
    pub gear: u8,
    pub engine_rpm: f32,
    pub shift_timer: f32,
    /// Filtered body-frame acceleration (x forward, y left), m/s².
    pub accel_local: Vec2,
    /// Body roll, rad; positive leans to the right (left side up).
    #[serde(default)]
    pub roll: f32,
    #[serde(default)]
    pub roll_rate: f32,
    /// Body pitch, rad; positive is nose down.
    #[serde(default)]
    pub pitch: f32,
    #[serde(default)]
    pub pitch_rate: f32,
    /// True while ABS is modulating any wheel.
    #[serde(default)]
    pub abs_active: bool,
    /// Air brake pressures, for vehicles with air brakes.
    #[serde(default)]
    pub air: Option<AirState>,
    /// Tipping about the outer wheels, rad (0 = all wheels down, π/2 = on its side).
    #[serde(default)]
    pub tip_angle: f32,
    #[serde(default)]
    pub tip_rate: f32,
    /// +1 tipping to the right, −1 to the left.
    #[serde(default)]
    pub tip_dir: f32,
    /// Set once the vehicle has tipped over.
    #[serde(default)]
    pub rolled_over: bool,
    /// Reverse gear engaged.
    #[serde(default)]
    pub reverse: bool,
    /// Velocity at the previous substep (world frame), for measuring acceleration.
    #[serde(default)]
    pub prev_velocity: Vec2,
    /// Vertical load from a coupled semi-trailer's kingpin, N (written by the trailer).
    #[serde(default)]
    pub hitch_load_n: f32,
    /// Brake application 0–1 after the air system: what a coupled trailer's brakes receive.
    #[serde(default)]
    pub brake_fraction: f32,
}

impl Component for Vehicle {
    const NAME: &'static str = "Vehicle";
}

impl Vehicle {
    pub fn new(def: &VehicleDef) -> Self {
        Self {
            model: def.name.clone(),
            wheels: std::array::from_fn(|_| WheelState {
                abs_factor: 1.0,
                ..Default::default()
            }),
            steer_angle: 0.0,
            gear: 1,
            engine_rpm: def.engine.idle_rpm,
            shift_timer: 0.0,
            accel_local: Vec2::ZERO,
            roll: 0.0,
            roll_rate: 0.0,
            pitch: 0.0,
            pitch_rate: 0.0,
            abs_active: false,
            air: def.brakes.air.as_ref().map(|a| AirState {
                reservoir_bar: a.reservoir_max_bar,
                chamber_bar: 0.0,
            }),
            tip_angle: 0.0,
            tip_rate: 0.0,
            tip_dir: 0.0,
            rolled_over: false,
            reverse: false,
            prev_velocity: Vec2::ZERO,
            hitch_load_n: 0.0,
            brake_fraction: 0.0,
        }
    }
}

/// Driver controls, each clamped by the dynamics system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VehicleInput {
    /// 0 to 1.
    pub throttle: f32,
    /// 0 to 1.
    pub brake: f32,
    /// −1 (full right) to 1 (full left).
    pub steer: f32,
    /// Request reverse gear. Engages (or disengages) only below 2 km/h.
    #[serde(default)]
    pub reverse: bool,
}

impl Component for VehicleInput {
    const NAME: &'static str = "VehicleInput";
}

/// Something worth knowing happened to a vehicle. Sent during physics,
/// readable after the tick (control protocol: `events.poll`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum VehicleEvent {
    GearShift {
        entity: String,
        from: u8,
        to: u8,
    },
    AbsEngaged {
        entity: String,
    },
    AbsReleased {
        entity: String,
    },
    WheelLift {
        entity: String,
        wheel: String,
    },
    WheelLanded {
        entity: String,
        wheel: String,
    },
    LowAirPressure {
        entity: String,
        bar: f32,
    },
    /// The centre of mass passed over the outer wheels: the vehicle is on its side.
    RolledOver {
        entity: String,
        roll_deg: f32,
    },
    /// The angle between tractor and trailer passed 60°.
    Jackknife {
        entity: String,
        angle_deg: f32,
    },
    /// This vehicle's body touched another vehicle or trailer (sent once per
    /// contact, to both).
    Collision {
        entity: String,
        other: String,
    },
}

impl VehicleEvent {
    pub const KINDS: [&'static str; 9] = [
        "GearShift",
        "AbsEngaged",
        "AbsReleased",
        "WheelLift",
        "WheelLanded",
        "LowAirPressure",
        "RolledOver",
        "Jackknife",
        "Collision",
    ];

    pub fn kind(&self) -> &'static str {
        match self {
            Self::GearShift { .. } => "GearShift",
            Self::AbsEngaged { .. } => "AbsEngaged",
            Self::AbsReleased { .. } => "AbsReleased",
            Self::WheelLift { .. } => "WheelLift",
            Self::WheelLanded { .. } => "WheelLanded",
            Self::LowAirPressure { .. } => "LowAirPressure",
            Self::RolledOver { .. } => "RolledOver",
            Self::Jackknife { .. } => "Jackknife",
            Self::Collision { .. } => "Collision",
        }
    }

    /// The vehicle the event is about.
    pub fn entity(&self) -> &str {
        match self {
            Self::GearShift { entity, .. }
            | Self::AbsEngaged { entity }
            | Self::AbsReleased { entity }
            | Self::WheelLift { entity, .. }
            | Self::WheelLanded { entity, .. }
            | Self::LowAirPressure { entity, .. }
            | Self::RolledOver { entity, .. }
            | Self::Jackknife { entity, .. }
            | Self::Collision { entity, .. } => entity,
        }
    }
}

/// All vehicle definitions available to the simulation, by name.
#[derive(Default)]
pub struct VehicleLibrary(pub BTreeMap<String, VehicleDef>);

pub struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn name(&self) -> &'static str {
        "vehicle"
    }
    fn dependencies(&self) -> Vec<&'static str> {
        vec!["physics"]
    }
    fn build(&self, app: &mut App) {
        app.world.register::<Vehicle>();
        app.world.register::<VehicleInput>();
        if !app.world.has_resource::<VehicleLibrary>() {
            app.world.insert_resource(VehicleLibrary::default());
        }
        app.add_event::<VehicleEvent>("VehicleEvent");
        app.add_system(Stage::Physics, "vehicle_dynamics", vehicle_dynamics)
            .before("integrate_velocities")
            .reads::<VehicleInput>()
            .reads::<Pose>()
            .reads::<Velocity>()
            .writes::<Vehicle>()
            .writes::<RigidBody>()
            .reads_resource("VehicleLibrary")
            .writes_resource("Events<VehicleEvent>");
        app.add_system(Stage::Post, "detect_collisions", detect_collisions)
            .reads::<Pose>()
            .reads::<wp_physics::Collider>()
            .writes_resource("Contacts")
            .writes_resource("Events<VehicleEvent>");
    }
}

/// Spawn a vehicle at rest. Adds its definition to the library.
pub fn spawn_vehicle(world: &mut World, def: &VehicleDef, position: Vec2, heading: f32) -> Entity {
    if !world.has_resource::<VehicleLibrary>() {
        world.insert_resource(VehicleLibrary::default());
    }
    world
        .resource_mut::<VehicleLibrary>()
        .0
        .insert(def.name.clone(), def.clone());
    let e = world.spawn();
    world.insert(e, Pose { position, heading });
    world.insert(e, Velocity::default());
    world.insert(e, RigidBody::new(def.mass_kg, def.yaw_inertia_kgm2));
    world.insert(e, Vehicle::new(def));
    world.insert(e, VehicleInput::default());
    world.insert(e, vehicle_collider(def));
    e
}

/// The physics box for a vehicle's body.
pub fn vehicle_collider(def: &VehicleDef) -> wp_physics::Collider {
    let fp = vehicle_footprint(def);
    wp_physics::Collider {
        front: fp.front,
        rear: fp.rear,
        half_width: fp.width / 2.0,
    }
}

/// Set a vehicle rolling straight ahead at `speed` m/s, wheels turning to
/// match and the gearbox in the gear the automatic would choose.
pub fn set_rolling_speed(world: &mut World, e: Entity, speed: f32) {
    let Some(pose) = world.get::<Pose>(e).map(|p| *p) else {
        return;
    };
    let Some(def) = world
        .get::<Vehicle>(e)
        .and_then(|v| world.resource::<VehicleLibrary>().0.get(&v.model).cloned())
    else {
        return;
    };
    if let Some(mut vel) = world.get_mut::<Velocity>(e) {
        vel.linear = pose.forward() * speed;
        vel.angular = 0.0;
    }
    if let Some(mut veh) = world.get_mut::<Vehicle>(e) {
        veh.prev_velocity = pose.forward() * speed;
        let omega = speed / def.wheel.radius_m;
        for w in &mut veh.wheels {
            w.omega = omega;
            w.slip_ratio = 0.0;
            w.slip_tan = 0.0;
        }
        let wheel_rpm = omega * 60.0 / std::f32::consts::TAU;
        let gear = def
            .gearbox
            .ratios
            .iter()
            .position(|r| wheel_rpm * r * def.gearbox.final_drive < def.gearbox.shift_up_rpm)
            .unwrap_or(def.gearbox.ratios.len().saturating_sub(1));
        veh.gear = gear as u8 + 1;
    }
}

/// A snapshot of what a driver or test cares about.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Telemetry {
    pub speed_kmh: f32,
    /// Signed speed along the vehicle's heading, km/h.
    pub forward_kmh: f32,
    /// Lateral acceleration in g, positive to the left.
    pub lateral_g: f32,
    /// Longitudinal acceleration in g, positive forward.
    pub longitudinal_g: f32,
    pub yaw_rate_deg_s: f32,
    /// Angle between where the vehicle points and where it travels, degrees
    /// (+ sliding to the left); 0 below 10 km/h. Large values mean a slide or spin.
    pub sideslip_deg: f32,
    pub gear: u8,
    pub engine_rpm: f32,
    pub steer_deg: f32,
    pub position: [f32; 2],
    pub heading_deg: f32,
    /// Body roll (positive leans right) and pitch (positive nose down), degrees.
    pub roll_deg: f32,
    pub pitch_deg: f32,
    pub abs_active: bool,
    /// Reverse gear engaged.
    pub reverse: bool,
    /// Air reservoir pressure, bar (air brakes only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_reservoir_bar: Option<f32>,
    /// Wheel loads FL, FR, RL, RR, newtons.
    pub wheel_loads_n: [f32; 4],
}

/// Sideslip angle, rad: from the heading to the velocity, + left; 0 when slow.
fn sideslip(forward: glam::Vec2, velocity: glam::Vec2) -> f32 {
    if velocity.length() < 10.0 / 3.6 {
        return 0.0;
    }
    // Measured against the direction of travel along the body, so reversing is not a slide.
    let along = if velocity.dot(forward) < 0.0 {
        -forward
    } else {
        forward
    };
    along.angle_to(velocity)
}

pub fn telemetry(world: &World, e: Entity) -> Option<Telemetry> {
    let pose = *world.get::<Pose>(e)?;
    let vel = *world.get::<Velocity>(e)?;
    let veh = world.get::<Vehicle>(e)?;
    let g = world.get_resource::<Gravity>().map(|g| g.0).unwrap_or(9.81);
    Some(Telemetry {
        speed_kmh: vel.linear.length() * 3.6,
        forward_kmh: vel.linear.dot(pose.forward()) * 3.6,
        lateral_g: veh.accel_local.y / g,
        longitudinal_g: veh.accel_local.x / g,
        yaw_rate_deg_s: vel.angular.to_degrees(),
        sideslip_deg: sideslip(pose.forward(), vel.linear).to_degrees(),
        gear: veh.gear,
        engine_rpm: veh.engine_rpm,
        steer_deg: veh.steer_angle.to_degrees(),
        position: pose.position.to_array(),
        heading_deg: pose.heading_deg(),
        roll_deg: (veh.roll + veh.tip_angle * veh.tip_dir).to_degrees(),
        pitch_deg: veh.pitch.to_degrees(),
        abs_active: veh.abs_active,
        reverse: veh.reverse,
        air_reservoir_bar: veh.air.map(|a| a.reservoir_bar),
        wheel_loads_n: std::array::from_fn(|i| veh.wheels[i].load),
    })
}
