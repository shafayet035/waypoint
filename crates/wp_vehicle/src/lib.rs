//! Vehicles: the `.vehicle.ron` format and the vehicle dynamics system.
//!
//! A vehicle entity has [`Pose`], [`Velocity`] and [`RigidBody`] from
//! `wp_physics`, plus [`Vehicle`] (part state) and [`VehicleInput`] (driver
//! controls). Each physics substep, [`vehicle_dynamics`] computes wheel loads,
//! tyre slip, tyre forces, engine and brake torques, and adds the resulting
//! force and yaw moment to the rigid body.

pub mod def;
pub mod parts;

use glam::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wp_app::{App, Plugin};
use wp_core::SimTime;
use wp_ecs::{Component, Entity, Stage, World};
use wp_physics::{Gravity, Ground, Pose, RigidBody, Velocity};

pub use def::{Drive, VehicleDef};

/// Air density at sea level, kg/m³.
const AIR_DENSITY: f32 = 1.225;
/// Added to |vx| in the slip equations so they stay stable at standstill, m/s.
const SLIP_DAMPING_SPEED: f32 = 0.5;
/// Below this speed a contact-patch damper settles the tyres, m/s.
const LOW_SPEED_DAMPING_BELOW: f32 = 3.0;
/// Contact-patch damping per newton of load, N·s/m per N.
const CONTACT_DAMPING_PER_LOAD: f32 = 1.5;
/// Seconds between automatic gear changes.
const SHIFT_COOLDOWN_S: f32 = 0.6;
/// Time constant of the acceleration filter used for load transfer, s.
const ACCEL_FILTER_S: f32 = 0.03;

/// Wheel order used everywhere: front-left, front-right, rear-left, rear-right.
pub const WHEEL_NAMES: [&str; 4] = ["FL", "FR", "RL", "RR"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WheelState {
    /// Spin rate, rad/s.
    pub omega: f32,
    /// Relaxed longitudinal slip κ.
    pub slip_ratio: f32,
    /// Relaxed lateral slip tan α.
    pub slip_tan: f32,
    /// Vertical load, N.
    pub load: f32,
    /// Tyre force in the wheel frame (x forward, y left), N.
    pub force: Vec2,
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
}

impl Component for Vehicle {
    const NAME: &'static str = "Vehicle";
}

impl Vehicle {
    pub fn new(model: &str, idle_rpm: f32) -> Self {
        Self {
            model: model.to_string(),
            wheels: Default::default(),
            steer_angle: 0.0,
            gear: 1,
            engine_rpm: idle_rpm,
            shift_timer: 0.0,
            accel_local: Vec2::ZERO,
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
}

impl Component for VehicleInput {
    const NAME: &'static str = "VehicleInput";
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
        app.add_system(Stage::Physics, "vehicle_dynamics", vehicle_dynamics)
            .before("integrate_bodies")
            .reads::<VehicleInput>()
            .reads::<Pose>()
            .reads::<Velocity>()
            .writes::<Vehicle>()
            .writes::<RigidBody>()
            .reads_resource("VehicleLibrary");
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
    world.insert(e, Vehicle::new(&def.name, def.engine.idle_rpm));
    world.insert(e, VehicleInput::default());
    e
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

/// One physics substep for every vehicle.
pub fn vehicle_dynamics(world: &mut World) {
    let dt = world.resource::<SimTime>().physics_dt();
    let g = world.resource::<Gravity>().0;
    let surface = world
        .get_resource::<Ground>()
        .map(|gr| gr.friction_scale)
        .unwrap_or(1.0);
    let library = world.resource::<VehicleLibrary>();
    world
        .query::<(
            &mut Vehicle,
            &VehicleInput,
            &Pose,
            &Velocity,
            &mut RigidBody,
        )>()
        .for_each(|_, (veh, input, pose, vel, body)| {
            if let Some(def) = library.0.get(&veh.model) {
                step_vehicle(def, veh, input, pose, vel, body, dt, g, surface);
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn step_vehicle(
    def: &VehicleDef,
    veh: &mut Vehicle,
    input: &VehicleInput,
    pose: &Pose,
    vel: &Velocity,
    body: &mut RigidBody,
    dt: f32,
    g: f32,
    surface: f32,
) {
    let m = def.mass_kg;
    let a = def.cg_to_front_axle_m;
    let b = def.rear_axle_from_cg();
    let len = def.wheelbase_m;
    let h = def.cg_height_m;
    let half_track = def.track_width_m / 2.0;
    let radius = def.wheel.radius_m;
    let mu = def.tyre.friction * surface;

    // Steering, rate-limited.
    let target = input.steer.clamp(-1.0, 1.0) * def.steering.max_angle_deg.to_radians();
    let max_step = def.steering.rate_deg_per_s.to_radians() * dt;
    veh.steer_angle += (target - veh.steer_angle).clamp(-max_step, max_step);

    // Body-frame motion.
    let v_local = pose.to_local(vel.linear);
    let yaw_rate = vel.angular;

    // Wheel loads with longitudinal and lateral transfer from filtered acceleration.
    let ax = veh.accel_local.x;
    let ay = veh.accel_local.y;
    let front_axle = (m * g * b / len - m * ax * h / len).max(0.0);
    let rear_axle = (m * g - front_axle).max(0.0);
    let split = |axle: f32| {
        let d = axle * ay * h / (g * def.track_width_m);
        ((axle - d) / 2.0, (axle + d) / 2.0) // (left, right): left turn (+ay) loads the right side
    };
    let (fl, fr) = split(front_axle);
    let (rl, rr) = split(rear_axle);
    let loads = [fl.max(0.0), fr.max(0.0), rl.max(0.0), rr.max(0.0)];
    let positions = [
        Vec2::new(a, half_track),
        Vec2::new(a, -half_track),
        Vec2::new(-b, half_track),
        Vec2::new(-b, -half_track),
    ];
    let steer = [veh.steer_angle, veh.steer_angle, 0.0, 0.0];

    // Drivetrain: open differential, automatic gearbox.
    let driven = match def.drive {
        Drive::Front => [true, true, false, false],
        Drive::Rear => [false, false, true, true],
        Drive::All => [true; 4],
    };
    let n_driven = driven.iter().filter(|d| **d).count() as f32;
    let gears = &def.gearbox.ratios;
    veh.gear = veh.gear.clamp(1, gears.len().max(1) as u8);
    let ratio = gears.get(veh.gear as usize - 1).copied().unwrap_or(1.0) * def.gearbox.final_drive;
    let avg_omega: f32 = (0..4)
        .filter(|&i| driven[i])
        .map(|i| veh.wheels[i].omega)
        .sum::<f32>()
        / n_driven.max(1.0);
    let rpm_from_wheels = avg_omega * ratio * 60.0 / std::f32::consts::TAU;
    let engine = &def.engine;
    veh.engine_rpm = rpm_from_wheels.max(engine.idle_rpm).min(engine.redline_rpm);
    let throttle = input.throttle.clamp(0.0, 1.0);
    let engine_torque = if rpm_from_wheels >= engine.redline_rpm {
        0.0
    } else if throttle > 0.02 {
        parts::curve_torque(&engine.torque_curve, veh.engine_rpm) * throttle
    } else if rpm_from_wheels > engine.idle_rpm {
        -engine.engine_brake_nm * (rpm_from_wheels / engine.redline_rpm)
    } else {
        0.0
    };
    let wheel_drive_torque = engine_torque * ratio * def.gearbox.efficiency / n_driven.max(1.0);

    // Shift on road speed, not wheel speed, so wheelspin cannot cause gear hunting
    // (the way a real gearbox controller cross-checks wheel speeds).
    let road_rpm = v_local.x.max(0.0) / radius * ratio * 60.0 / std::f32::consts::TAU;
    veh.shift_timer = (veh.shift_timer - dt).max(0.0);
    if veh.shift_timer == 0.0 {
        if road_rpm > def.gearbox.shift_up_rpm && (veh.gear as usize) < gears.len() {
            veh.gear += 1;
            veh.shift_timer = SHIFT_COOLDOWN_S;
        } else if road_rpm < def.gearbox.shift_down_rpm && veh.gear > 1 {
            veh.gear -= 1;
            veh.shift_timer = SHIFT_COOLDOWN_S;
        }
    }

    // Brakes.
    let brake = input.brake.clamp(0.0, 1.0) * def.brakes.max_torque_nm;
    let front_brake = brake * def.brakes.front_bias / 2.0;
    let rear_brake = brake * (1.0 - def.brakes.front_bias) / 2.0;
    let brake_torque = [front_brake, front_brake, rear_brake, rear_brake];

    // Tyres.
    let relax = def.tyre.relaxation_length_m;
    let mut force = Vec2::ZERO;
    let mut moment = 0.0;
    for i in 0..4 {
        let p = positions[i];
        let contact_vel = v_local + Vec2::new(-yaw_rate * p.y, yaw_rate * p.x);
        let wheel_rot = Vec2::from_angle(steer[i]);
        let vt = Vec2::from_angle(-steer[i]).rotate(contact_vel);
        let w = &mut veh.wheels[i];

        // Relaxation length shrinks near standstill (down to 10%) so little
        // carcass wind-up is stored when the car stops.
        let relax_eff = relax * (vt.x.abs() / LOW_SPEED_DAMPING_BELOW).clamp(0.1, 1.0);
        let damping_speed = vt.x.abs() + SLIP_DAMPING_SPEED;
        w.slip_ratio += ((w.omega * radius - vt.x) - damping_speed * w.slip_ratio) * dt / relax_eff;
        w.slip_tan += (vt.y - damping_speed * w.slip_tan) * dt / relax_eff;

        (w.slip_ratio, w.slip_tan) =
            parts::cap_slip(mu, def.tyre.stiffness_per_load, w.slip_ratio, w.slip_tan);

        let (mut fx, mut fy) = parts::brush_tyre(
            loads[i],
            mu,
            def.tyre.stiffness_per_load,
            w.slip_ratio,
            w.slip_tan,
        );
        // Near standstill, damp contact-patch sliding so a stopped car settles
        // instead of rocking on its tyres. Fades out by LOW_SPEED_DAMPING_BELOW.
        let fade = (1.0 - vt.x.abs() / LOW_SPEED_DAMPING_BELOW).max(0.0);
        if fade > 0.0 {
            let c = CONTACT_DAMPING_PER_LOAD * loads[i] * fade;
            fx += c * (w.omega * radius - vt.x);
            fy -= c * vt.y;
            let limit = mu * loads[i];
            let f = (fx * fx + fy * fy).sqrt();
            if f > limit {
                fx *= limit / f;
                fy *= limit / f;
            }
        }
        let drive = if driven[i] { wheel_drive_torque } else { 0.0 };
        let resist = brake_torque[i] + def.tyre.rolling_resistance * loads[i] * radius;
        w.omega = parts::step_wheel(
            w.omega,
            drive,
            fx * radius,
            resist,
            def.wheel.inertia_kgm2,
            dt,
        );
        w.load = loads[i];
        w.force = Vec2::new(fx, fy);

        let fb = wheel_rot.rotate(w.force);
        force += fb;
        moment += p.x * fb.y - p.y * fb.x;
    }

    // Aerodynamic drag.
    let speed = v_local.length();
    force += -0.5 * AIR_DENSITY * def.aero.drag_area_m2 * speed * v_local;

    // Filtered acceleration drives next substep's load transfer.
    let alpha = dt / (ACCEL_FILTER_S + dt);
    veh.accel_local += (force / m - veh.accel_local) * alpha;

    body.apply_force(pose.to_world(force));
    body.apply_torque(moment);
}

/// A snapshot of what a driver or test cares about.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Telemetry {
    pub speed_kmh: f32,
    /// Signed speed along the vehicle's heading, km/h.
    pub forward_kmh: f32,
    /// Lateral acceleration in g, positive to the left.
    pub lateral_g: f32,
    /// Longitudinal acceleration in g, positive forward.
    pub longitudinal_g: f32,
    pub yaw_rate_deg_s: f32,
    pub gear: u8,
    pub engine_rpm: f32,
    pub steer_deg: f32,
    pub position: [f32; 2],
    pub heading_deg: f32,
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
        gear: veh.gear,
        engine_rpm: veh.engine_rpm,
        steer_deg: veh.steer_angle.to_degrees(),
        position: pose.position.to_array(),
        heading_deg: pose.heading_deg(),
    })
}
