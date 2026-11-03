//! Semi-trailers: `.trailer.ron` definitions and their dynamics.
//!
//! A trailer is its own rigid body joined to a tractor's fifth wheel by a
//! [`PinJoint`]. Each substep, [`trailer_dynamics`] computes axle and kingpin
//! loads, runs every trailer tyre with the same tyre model as vehicles, applies
//! brakes from the tractor's brake valve (with optional ABS), and reports the
//! kingpin load to the tractor so its axles carry it.

use crate::def::{AeroDef, TyreDef, WheelDef};
use crate::{Vehicle, VehicleEvent, VehicleLibrary, WheelState, dynamics::tyre_contact, parts};
use glam::Vec2;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wp_core::{Diagnostic, SimTime};
use wp_data::Loaded;
use wp_ecs::{Component, Entity, Events, World};
use wp_physics::{Gravity, Ground, PinJoint, Pose, RigidBody, Velocity};

/// Articulation beyond this is reported as a jackknife, degrees.
pub const JACKKNIFE_DEG: f32 = 60.0;

/// A semi-trailer: `trailers/<name>.trailer.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrailerDef {
    pub name: String,
    /// Total mass including load, kg.
    pub mass_kg: f32,
    pub yaw_inertia_kgm2: f32,
    pub cg_height_m: f32,
    /// Centre of mass behind the kingpin, m.
    pub cg_behind_kingpin_m: f32,
    /// Each axle's distance behind the kingpin, m, front axle first.
    pub axles_behind_kingpin_m: Vec<f32>,
    pub track_width_m: f32,
    pub wheel: WheelDef,
    pub tyre: TyreDef,
    pub brakes: TrailerBrakeDef,
    pub aero: AeroDef,
    pub body: TrailerBodyDef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrailerBrakeDef {
    /// Total brake torque of all trailer wheels at full application, N·m.
    pub max_torque_nm: f32,
    #[serde(default)]
    pub abs: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrailerBodyDef {
    pub length_m: f32,
    pub width_m: f32,
    pub height_m: f32,
    /// From the front of the body back to the kingpin, m.
    pub kingpin_from_front_m: f32,
}

impl TrailerDef {
    pub fn mean_axle_m(&self) -> f32 {
        self.axles_behind_kingpin_m.iter().sum::<f32>()
            / self.axles_behind_kingpin_m.len().max(1) as f32
    }

    pub fn validate(file: &Loaded<TrailerDef>) -> Vec<Diagnostic> {
        let t = &file.value;
        let mut out = Vec::new();
        let positive = [
            ("mass_kg", t.mass_kg),
            ("yaw_inertia_kgm2", t.yaw_inertia_kgm2),
            ("cg_height_m", t.cg_height_m),
            ("track_width_m", t.track_width_m),
            ("radius_m", t.wheel.radius_m),
            ("inertia_kgm2", t.wheel.inertia_kgm2),
        ];
        for (key, v) in positive {
            if !(v > 0.0 && v.is_finite()) {
                out.push(
                    file.error_at("WP-T001", key, format!("`{key}` must be a positive number"))
                        .expected(format!("{key} > 0, found {v}")),
                );
            }
        }
        let axles = &t.axles_behind_kingpin_m;
        if axles.is_empty()
            || axles.iter().any(|a| !(*a > 0.0))
            || axles.windows(2).any(|w| w[1] <= w[0])
        {
            out.push(
                file.error_at(
                    "WP-T002",
                    "axles_behind_kingpin_m",
                    "axles must be listed front to back, all behind the kingpin",
                )
                .help("e.g. axles_behind_kingpin_m: [6.39, 7.7, 9.01]"),
            );
        } else if !(t.cg_behind_kingpin_m > 0.0 && t.cg_behind_kingpin_m < t.mean_axle_m()) {
            out.push(
                file.error_at(
                    "WP-T003",
                    "cg_behind_kingpin_m",
                    "the centre of mass must lie between the kingpin and the axles",
                )
                .expected(format!(
                    "0 < cg_behind_kingpin_m < mean axle distance ({:.2})",
                    t.mean_axle_m()
                )),
            );
        }
        let b = &t.body;
        if !(b.length_m > 0.0 && b.width_m > 0.0 && b.height_m > 0.0)
            || !(b.kingpin_from_front_m >= 0.0 && b.kingpin_from_front_m < b.length_m)
            || axles
                .last()
                .is_some_and(|a| b.kingpin_from_front_m + a > b.length_m)
        {
            out.push(
                file.error_at("WP-T004", "body", "body dimensions do not fit the axles")
                    .expected(
                        "0 ≤ kingpin_from_front_m < length_m, and the last axle inside the body",
                    ),
            );
        }
        out
    }
}

/// Semi-trailers: coupling, axle loads, tyres and brakes. Needs `vehicle`.
pub struct TrailerPlugin;

impl wp_app::Plugin for TrailerPlugin {
    fn name(&self) -> &'static str {
        "trailer"
    }
    fn dependencies(&self) -> Vec<&'static str> {
        vec!["vehicle"]
    }
    fn build(&self, app: &mut wp_app::App) {
        app.world.register::<Trailer>();
        if !app.world.has_resource::<TrailerLibrary>() {
            app.world.insert_resource(TrailerLibrary::default());
        }
        // Trailers first: they set the tractor's kingpin load for this substep.
        app.add_system(wp_ecs::Stage::Physics, "trailer_dynamics", trailer_dynamics)
            .before("vehicle_dynamics")
            .reads::<Pose>()
            .reads::<Velocity>()
            .writes::<Trailer>()
            .writes::<RigidBody>()
            .writes::<Vehicle>()
            .reads_resource("TrailerLibrary");
    }
}

/// Runtime state of a trailer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trailer {
    pub model: String,
    /// Two wheels per axle: [axle0 left, axle0 right, axle1 left, ...].
    pub wheels: Vec<WheelState>,
    #[serde(default)]
    pub prev_velocity: Vec2,
    /// Filtered body-frame acceleration, m/s².
    #[serde(default)]
    pub accel_local: Vec2,
    /// Tractor heading minus trailer heading, rad (positive: trailer swung right).
    #[serde(default)]
    pub articulation: f32,
    #[serde(default)]
    pub jackknifed: bool,
    #[serde(default)]
    pub braking: bool,
}

impl Component for Trailer {
    const NAME: &'static str = "Trailer";
}

/// All trailer definitions available to the simulation, by name.
#[derive(Default)]
pub struct TrailerLibrary(pub BTreeMap<String, TrailerDef>);

/// Spawn `def` coupled to `tractor`'s fifth wheel, lined up straight behind it.
/// Returns `None` if the tractor has no hitch.
pub fn spawn_trailer(world: &mut World, def: &TrailerDef, tractor: Entity) -> Option<Entity> {
    let tpose = *world.get::<Pose>(tractor)?;
    let tvel = *world.get::<Velocity>(tractor)?;
    let vdef = {
        let veh = world.get::<Vehicle>(tractor)?;
        world
            .resource::<VehicleLibrary>()
            .0
            .get(&veh.model)?
            .clone()
    };
    let hitch = vdef.hitch.as_ref()?;
    let fifth_wheel = Vec2::new(-vdef.rear_axle_from_cg() + hitch.ahead_of_rear_axle_m, 0.0);
    let kingpin_world = tpose.position + tpose.to_world(fifth_wheel);
    let position = kingpin_world - tpose.forward() * def.cg_behind_kingpin_m;

    if !world.has_resource::<TrailerLibrary>() {
        world.insert_resource(TrailerLibrary::default());
    }
    world
        .resource_mut::<TrailerLibrary>()
        .0
        .insert(def.name.clone(), def.clone());
    let e = world.spawn();
    world.insert(
        e,
        Pose {
            position,
            heading: tpose.heading,
        },
    );
    world.insert(
        e,
        Velocity {
            linear: tvel.linear,
            angular: 0.0,
        },
    );
    world.insert(e, RigidBody::new(def.mass_kg, def.yaw_inertia_kgm2));
    let speed = tvel.linear.length();
    let wheels = (0..def.axles_behind_kingpin_m.len() * 2)
        .map(|_| WheelState {
            omega: speed / def.wheel.radius_m,
            abs_factor: 1.0,
            ..Default::default()
        })
        .collect();
    world.insert(
        e,
        Trailer {
            model: def.name.clone(),
            wheels,
            prev_velocity: tvel.linear,
            accel_local: Vec2::ZERO,
            articulation: 0.0,
            jackknifed: false,
            braking: false,
        },
    );
    world.insert(
        e,
        PinJoint {
            other: tractor,
            local_anchor: Vec2::new(def.cg_behind_kingpin_m, 0.0),
            other_anchor: fifth_wheel,
        },
    );
    Some(e)
}

/// One physics substep for every trailer.
pub fn trailer_dynamics(world: &mut World) {
    let dt = world.resource::<SimTime>().physics_dt();
    let g = world.resource::<Gravity>().0;
    let surface = world
        .get_resource::<Ground>()
        .map(|gr| gr.friction_scale)
        .unwrap_or(1.0);
    let Some(library) = world.get_resource::<TrailerLibrary>() else {
        return;
    };
    let mut events = Vec::new();
    world
        .query::<(&mut Trailer, &PinJoint, &Pose, &Velocity, &mut RigidBody)>()
        .for_each(|_, (tr, joint, pose, vel, body)| {
            let Some(def) = library.0.get(&tr.model) else {
                return;
            };
            let tractor = joint.other;
            let tractor_heading = world
                .get::<Pose>(tractor)
                .map(|p| p.heading)
                .unwrap_or(pose.heading);
            let brake_fraction = world
                .get::<Vehicle>(tractor)
                .map(|v| v.brake_fraction)
                .unwrap_or(0.0);
            let kingpin_load =
                step_trailer(def, tr, pose, vel, body, brake_fraction, dt, g, surface);
            if let Some(mut veh) = world.get_mut::<Vehicle>(tractor) {
                veh.hitch_load_n = kingpin_load;
            }
            // Articulation, wrapped to (−π, π].
            let pi = std::f32::consts::PI;
            tr.articulation = (tractor_heading - pose.heading + pi).rem_euclid(2.0 * pi) - pi;
            let jack = tr.articulation.abs() > JACKKNIFE_DEG.to_radians();
            if jack && !tr.jackknifed {
                events.push(VehicleEvent::Jackknife {
                    entity: tractor.to_string(),
                    angle_deg: tr.articulation.to_degrees(),
                });
            }
            tr.jackknifed = jack;
        });
    drop(library);
    if let Some(mut ev) = world.get_resource_mut::<Events<VehicleEvent>>() {
        for e in events {
            ev.send(e);
        }
    }
}

/// Returns the vertical load on the kingpin, N.
#[allow(clippy::too_many_arguments)]
fn step_trailer(
    def: &TrailerDef,
    tr: &mut Trailer,
    pose: &Pose,
    vel: &Velocity,
    body: &mut RigidBody,
    brake_fraction: f32,
    dt: f32,
    g: f32,
    surface: f32,
) -> f32 {
    let m = def.mass_kg;
    let h = def.cg_height_m;
    let d_cg = def.cg_behind_kingpin_m;
    let d_ax = def.mean_axle_m();
    let n_axles = def.axles_behind_kingpin_m.len();
    let half_track = def.track_width_m / 2.0;
    let radius = def.wheel.radius_m;

    let a_world = (vel.linear - tr.prev_velocity) / dt;
    tr.prev_velocity = vel.linear;
    let alpha = dt / (0.03 + dt);
    tr.accel_local += (pose.to_local(a_world) - tr.accel_local) * alpha;
    let (ax, ay) = (tr.accel_local.x, tr.accel_local.y);

    // Loads: weight shared between kingpin and axle group; braking (ax < 0)
    // pushes load forward onto the kingpin. Lateral transfer is instant.
    let weight = m * g;
    let kingpin = (weight * (d_ax - d_cg) / d_ax - m * ax * h / d_ax).clamp(0.0, weight);
    let per_axle = (weight - kingpin) / n_axles as f32;
    let transfer =
        m * ay * h / def.track_width_m * (per_axle * n_axles as f32 / weight) / n_axles as f32;
    let v_local = pose.to_local(vel.linear);
    let speed = v_local.length();
    let brake_per_wheel = brake_fraction * def.brakes.max_torque_nm / (2 * n_axles) as f32;
    tr.braking = brake_fraction > 0.05;
    let peak_mu = def.tyre.friction * surface;

    let mut force = Vec2::ZERO;
    let mut moment = 0.0;
    for (axle, dist) in def.axles_behind_kingpin_m.iter().enumerate() {
        let x = d_cg - dist; // axle position relative to the centre of mass
        for side in 0..2 {
            let y = if side == 0 { half_track } else { -half_track };
            // Left turn (ay > 0) loads the right side.
            let load = (per_axle / 2.0 + if side == 0 { -transfer } else { transfer }).max(0.0);
            let w = &mut tr.wheels[axle * 2 + side];
            let vt = v_local + Vec2::new(-vel.angular * y, vel.angular * x);
            let (fx, fy, kin_long) = tyre_contact(w, vt, load, peak_mu, &def.tyre, radius, dt);
            w.abs_factor = if def.brakes.abs {
                parts::abs_step(w.abs_factor, kin_long, brake_per_wheel > 0.0, speed, dt)
            } else {
                1.0
            };
            let resist =
                brake_per_wheel * w.abs_factor + def.tyre.rolling_resistance * load * radius;
            w.omega = parts::step_wheel(
                w.omega,
                0.0,
                fx * radius,
                resist,
                def.wheel.inertia_kgm2,
                dt,
            );
            w.angle = (w.angle + w.omega * dt).rem_euclid(std::f32::consts::TAU);
            w.load = load;
            w.lifted = load <= 0.0;
            w.force = Vec2::new(fx, fy);
            force += w.force;
            moment += x * fy - y * fx;
        }
    }
    force += -0.5 * 1.225 * def.aero.drag_area_m2 * v_local.length() * v_local;
    body.apply_force(pose.to_world(force));
    body.apply_torque(moment);
    kingpin
}
