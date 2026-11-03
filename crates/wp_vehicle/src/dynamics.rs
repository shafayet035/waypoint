//! One physics substep of the vehicle model.

use crate::def::{Drive, VehicleDef};
use crate::{Vehicle, VehicleEvent, VehicleInput, VehicleLibrary, WHEEL_NAMES, parts};
use glam::Vec2;
use wp_core::SimTime;
use wp_ecs::{Events, World};
use wp_physics::{Gravity, Ground, Pose, RigidBody, Velocity};

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
/// Share of the mass carried by the springs (the rest is wheels and axles).
const SPRUNG_SHARE: f32 = 0.88;

/// One physics substep for every vehicle.
pub fn vehicle_dynamics(world: &mut World) {
    let dt = world.resource::<SimTime>().physics_dt();
    let g = world.resource::<Gravity>().0;
    let surface = world
        .get_resource::<Ground>()
        .map(|gr| gr.friction_scale)
        .unwrap_or(1.0);
    let library = world.resource::<VehicleLibrary>();
    let mut events = world.get_resource_mut::<Events<VehicleEvent>>();
    let mut local_events = Vec::new();
    world
        .query::<(
            &mut Vehicle,
            &VehicleInput,
            &Pose,
            &Velocity,
            &mut RigidBody,
        )>()
        .for_each(|e, (veh, input, pose, vel, body)| {
            if let Some(def) = library.0.get(&veh.model) {
                let ctx = Ctx {
                    def,
                    dt,
                    g,
                    surface,
                    entity: e.to_string(),
                };
                step_vehicle(&ctx, veh, input, pose, vel, body, &mut local_events);
            }
        });
    if let Some(ev) = events.as_mut() {
        for e in local_events {
            ev.send(e);
        }
    }
}

struct Ctx<'a> {
    def: &'a VehicleDef,
    dt: f32,
    g: f32,
    surface: f32,
    entity: String,
}

fn step_vehicle(
    ctx: &Ctx,
    veh: &mut Vehicle,
    input: &VehicleInput,
    pose: &Pose,
    vel: &Velocity,
    body: &mut RigidBody,
    events: &mut Vec<VehicleEvent>,
) {
    let (def, dt, g) = (ctx.def, ctx.dt, ctx.g);
    let m = def.mass_kg;
    let a = def.cg_to_front_axle_m;
    let b = def.rear_axle_from_cg();
    let len = def.wheelbase_m;
    let h = def.cg_height_m;
    let track = def.track_width_m;
    let radius = def.wheel.radius_m;
    let peak_mu = def.tyre.friction * ctx.surface;

    // Steering, rate-limited.
    let target = input.steer.clamp(-1.0, 1.0) * def.steering.max_angle_deg.to_radians();
    let max_step = def.steering.rate_deg_per_s.to_radians() * dt;
    veh.steer_angle += (target - veh.steer_angle).clamp(-max_step, max_step);

    let v_local = pose.to_local(vel.linear);
    let yaw_rate = vel.angular;
    let (ax, ay) = (veh.accel_local.x, veh.accel_local.y);

    // ---- Suspension: body roll and pitch as damped springs ----------------
    let susp = &def.suspension;
    let ms = m * SPRUNG_SHARE;
    let mu_mass = m - ms;
    let hs = ((m * h - mu_mass * radius) / ms).max(susp.roll_center_height_m + 0.05);
    let h_roll = hs - susp.roll_center_height_m;
    let roll_grad = susp.roll_gradient_deg_per_g.to_radians() / g; // rad per m/s²
    let k_roll = ms * h_roll * (1.0 / roll_grad + g);
    let i_roll = ms * ((track + 0.25).powi(2) + (2.0 * hs).powi(2)) / 12.0;
    let c_roll = 2.0 * susp.damping_ratio * (k_roll * i_roll).sqrt();
    let roll_acc = (ms * ay * h_roll + ms * g * h_roll * veh.roll.sin()
        - k_roll * veh.roll
        - c_roll * veh.roll_rate)
        / i_roll;
    veh.roll_rate += roll_acc * dt;
    veh.roll += veh.roll_rate * dt;

    let pitch_grad = susp.pitch_gradient_deg_per_g.to_radians() / g;
    let k_pitch = ms * hs * (1.0 / pitch_grad + g);
    let i_pitch = ms * (len.powi(2) + (2.0 * hs).powi(2)) / 12.0;
    let c_pitch = 2.0 * susp.damping_ratio * (k_pitch * i_pitch).sqrt();
    let pitch_acc = (-ms * ax * hs - k_pitch * veh.pitch - c_pitch * veh.pitch_rate) / i_pitch;
    veh.pitch_rate += pitch_acc * dt;
    veh.pitch += veh.pitch_rate * dt;

    // ---- Wheel loads -----------------------------------------------------
    let front_axle = (m * g * b / len - m * ax * h / len).max(0.0);
    let rear_axle = (m * g - front_axle).max(0.0);
    // Lateral transfer: through the springs (from roll) and through the roll
    // centre and unsprung mass (instant). Positive moves load to the right.
    let elastic = k_roll * veh.roll + c_roll * veh.roll_rate;
    let geometric = ms * ay * susp.roll_center_height_m + mu_mass * ay * radius;
    let static_front = b / len;
    let front_transfer = (elastic * susp.front_roll_share + geometric * static_front) / track;
    let rear_transfer =
        (elastic * (1.0 - susp.front_roll_share) + geometric * (1.0 - static_front)) / track;
    let raw = [
        (front_axle - front_transfer) / 2.0,
        (front_axle + front_transfer) / 2.0,
        (rear_axle - rear_transfer) / 2.0,
        (rear_axle + rear_transfer) / 2.0,
    ];
    let mut loads = raw.map(|l| l.max(0.0));

    // Tipping: once lateral acceleration beats gravity's lever (|ay| > g·t/2h)
    // the vehicle pivots about its outer wheels. Rigid-body dynamics about that
    // edge decide whether it falls back or goes over.
    let half = track / 2.0;
    if veh.tip_angle <= 0.0 && ay.abs() > g * half / h {
        veh.tip_dir = ay.signum();
        veh.tip_angle = 1e-4;
        veh.tip_rate = 0.0;
    }
    if veh.tip_angle > 0.0 {
        let i_tip = i_roll + m * (h * h + half * half);
        let (sn, cs) = veh.tip_angle.sin_cos();
        let a_eff = if veh.rolled_over {
            0.0
        } else {
            ay * veh.tip_dir
        };
        let moment = m * a_eff * (h * cs + half * sn) - m * g * (half * cs - h * sn);
        veh.tip_rate += moment / i_tip * dt;
        veh.tip_angle += veh.tip_rate * dt;
        if veh.tip_angle <= 0.0 {
            veh.tip_angle = 0.0;
            veh.tip_rate = 0.0;
        } else if veh.tip_angle >= std::f32::consts::FRAC_PI_2 {
            veh.tip_angle = std::f32::consts::FRAC_PI_2;
            veh.tip_rate = 0.0;
        }
        if !veh.rolled_over && veh.tip_angle > half.atan2(h) {
            veh.rolled_over = true;
            events.push(VehicleEvent::RolledOver {
                entity: ctx.entity.clone(),
                roll_deg: (veh.roll + veh.tip_angle * veh.tip_dir).to_degrees(),
            });
        }
        // Inner wheels are in the air; the outer ones carry each axle.
        let (inner, outer) = if veh.tip_dir > 0.0 {
            ([0, 2], [1, 3])
        } else {
            ([1, 3], [0, 2])
        };
        loads[inner[0]] = 0.0;
        loads[inner[1]] = 0.0;
        loads[outer[0]] = front_axle;
        loads[outer[1]] = rear_axle;
    }
    if veh.rolled_over {
        loads = [0.0; 4];
    }
    for (i, w) in veh.wheels.iter_mut().enumerate() {
        let lifted = loads[i] <= 0.0;
        if lifted != w.lifted {
            let wheel = WHEEL_NAMES[i].to_string();
            events.push(if lifted {
                VehicleEvent::WheelLift {
                    entity: ctx.entity.clone(),
                    wheel,
                }
            } else {
                VehicleEvent::WheelLanded {
                    entity: ctx.entity.clone(),
                    wheel,
                }
            });
            w.lifted = lifted;
        }
    }

    let half_track = track / 2.0;
    let positions = [
        Vec2::new(a, half_track),
        Vec2::new(a, -half_track),
        Vec2::new(-b, half_track),
        Vec2::new(-b, -half_track),
    ];
    let steer = [veh.steer_angle, veh.steer_angle, 0.0, 0.0];

    // ---- Drivetrain: open differential, automatic gearbox ----------------
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
    let mut throttle = input.throttle.clamp(0.0, 1.0);
    if let Some(limit) = engine.governed_speed_kmh {
        let over = v_local.x * 3.6 - limit;
        throttle *= (1.0 - over / 3.0).clamp(0.0, 1.0); // fade out over the last 3 km/h
    }
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

    // Shift on road speed, not wheel speed, so wheelspin cannot cause hunting.
    let road_rpm = v_local.x.max(0.0) / radius * ratio * 60.0 / std::f32::consts::TAU;
    veh.shift_timer = (veh.shift_timer - dt).max(0.0);
    if veh.shift_timer == 0.0 {
        let from = veh.gear;
        if road_rpm > def.gearbox.shift_up_rpm && (veh.gear as usize) < gears.len() {
            veh.gear += 1;
        } else if road_rpm < def.gearbox.shift_down_rpm && veh.gear > 1 {
            veh.gear -= 1;
        }
        if veh.gear != from {
            veh.shift_timer = SHIFT_COOLDOWN_S;
            events.push(VehicleEvent::GearShift {
                entity: ctx.entity.clone(),
                from,
                to: veh.gear,
            });
        }
    }

    // ---- Brakes: pedal → (air system) → per-wheel torque → ABS -------------
    let pedal = input.brake.clamp(0.0, 1.0);
    let brake_fraction = match (&def.brakes.air, veh.air.as_mut()) {
        (Some(air), Some(state)) => {
            let was_ok = state.reservoir_bar >= air.warning_bar;
            let f = parts::air_brake_step(state, air, pedal, dt);
            if was_ok && state.reservoir_bar < air.warning_bar {
                events.push(VehicleEvent::LowAirPressure {
                    entity: ctx.entity.clone(),
                    bar: state.reservoir_bar,
                });
            }
            f
        }
        _ => pedal,
    };
    let brake = brake_fraction * def.brakes.max_torque_nm;
    let front_brake = brake * def.brakes.front_bias / 2.0;
    let rear_brake = brake * (1.0 - def.brakes.front_bias) / 2.0;
    let brake_torque = [front_brake, front_brake, rear_brake, rear_brake];
    let speed = v_local.length();

    // ---- Tyres ------------------------------------------------------------
    let relax = def.tyre.relaxation_length_m;
    let k_tyre = def.tyre.stiffness_per_load;
    let mut force = Vec2::ZERO;
    let mut moment = 0.0;
    let mut abs_any = false;
    for i in 0..4 {
        let p = positions[i];
        let contact_vel = v_local + Vec2::new(-yaw_rate * p.y, yaw_rate * p.x);
        let wheel_rot = Vec2::from_angle(steer[i]);
        let vt = Vec2::from_angle(-steer[i]).rotate(contact_vel);
        let w = &mut veh.wheels[i];

        // Sliding friction from the kinematic (instantaneous) slip.
        let ref_speed = vt.x.abs().max(1.0);
        let kin_long = (w.omega * radius - vt.x) / ref_speed;
        let kin_lat = vt.y / ref_speed;
        let slide = (kin_long * kin_long + kin_lat * kin_lat).sqrt();
        let mu = parts::sliding_friction(peak_mu, def.tyre.sliding_friction_ratio, k_tyre, slide);

        // Relaxed slip states; the relaxation length shrinks near standstill so
        // little carcass wind-up is stored when the vehicle stops.
        let relax_eff = relax * (vt.x.abs() / LOW_SPEED_DAMPING_BELOW).clamp(0.1, 1.0);
        let damping_speed = vt.x.abs() + SLIP_DAMPING_SPEED;
        w.slip_ratio += ((w.omega * radius - vt.x) - damping_speed * w.slip_ratio) * dt / relax_eff;
        w.slip_tan += (vt.y - damping_speed * w.slip_tan) * dt / relax_eff;
        (w.slip_ratio, w.slip_tan) = parts::cap_slip(mu, k_tyre, w.slip_ratio, w.slip_tan);

        let (mut fx, mut fy) = parts::brush_tyre(loads[i], mu, k_tyre, w.slip_ratio, w.slip_tan);
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

        if def.brakes.abs {
            w.abs_factor =
                parts::abs_step(w.abs_factor, kin_long, brake_torque[i] > 0.0, speed, dt);
            abs_any |= w.abs_factor < 0.999;
        } else {
            w.abs_factor = 1.0;
        }
        let drive = if driven[i] { wheel_drive_torque } else { 0.0 };
        let resist =
            brake_torque[i] * w.abs_factor + def.tyre.rolling_resistance * loads[i] * radius;
        w.omega = parts::step_wheel(
            w.omega,
            drive,
            fx * radius,
            resist,
            def.wheel.inertia_kgm2,
            dt,
        );
        w.angle = (w.angle + w.omega * dt).rem_euclid(std::f32::consts::TAU);
        w.load = loads[i];
        w.force = Vec2::new(fx, fy);

        let fb = wheel_rot.rotate(w.force);
        force += fb;
        moment += p.x * fb.y - p.y * fb.x;
    }
    if abs_any != veh.abs_active {
        veh.abs_active = abs_any;
        events.push(if abs_any {
            VehicleEvent::AbsEngaged {
                entity: ctx.entity.clone(),
            }
        } else {
            VehicleEvent::AbsReleased {
                entity: ctx.entity.clone(),
            }
        });
    }

    // On its side, the body scrapes along the ground.
    if veh.rolled_over {
        let speed = v_local.length();
        const BODY_FRICTION: f32 = 0.5;
        force += -BODY_FRICTION * m * g * v_local / speed.max(0.5);
        moment +=
            -BODY_FRICTION * m * g * yaw_rate * len / (yaw_rate.abs() * len).max(0.5) * len / 4.0;
    }

    // Aerodynamic drag.
    force += -0.5 * AIR_DENSITY * def.aero.drag_area_m2 * v_local.length() * v_local;

    // Filtered acceleration drives next substep's load transfer and body motion.
    let alpha = dt / (ACCEL_FILTER_S + dt);
    veh.accel_local += (force / m - veh.accel_local) * alpha;

    body.apply_force(pose.to_world(force));
    body.apply_torque(moment);
}

#[cfg(test)]
mod tests {
    use crate::{VehicleDef, VehicleEvent, VehicleInput, spawn_vehicle};
    use glam::Vec2;
    use wp_app::App;
    use wp_ecs::{Entity, Events};

    fn sandbox_vehicle(name: &str) -> VehicleDef {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sandbox");
        wp_data::load_ron(&root, &format!("vehicles/{name}.vehicle.ron"))
            .unwrap()
            .value
    }

    fn app_with(def: &VehicleDef) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugin(wp_physics::PhysicsPlugin)
            .add_plugin(crate::VehiclePlugin);
        app.finish().unwrap();
        let e = spawn_vehicle(&mut app.world, def, Vec2::ZERO, 0.0);
        (app, e)
    }

    /// Run `seconds` with `input`, returning the kinds of events seen.
    fn drive(app: &mut App, e: Entity, input: VehicleInput, seconds: f32) -> Vec<String> {
        *app.world.get_mut::<VehicleInput>(e).unwrap() = input;
        let mut kinds = Vec::new();
        for _ in 0..(seconds * 60.0) as usize {
            app.tick();
            kinds.extend(
                app.world
                    .resource::<Events<VehicleEvent>>()
                    .iter()
                    .map(|ev| ev.kind().to_string()),
            );
        }
        kinds
    }

    #[test]
    fn top_heavy_vehicle_lifts_wheels_and_rolls_over() {
        let mut def = sandbox_vehicle("hatchback");
        def.cg_height_m = 1.5;
        def.track_width_m = 1.3;
        def.tyre.friction = 1.4;
        def.steering.rate_deg_per_s = 400.0;
        let (mut app, e) = app_with(&def);
        crate::set_rolling_speed(&mut app.world, e, 60.0 / 3.6);
        let kinds = drive(
            &mut app,
            e,
            VehicleInput {
                throttle: 0.3,
                brake: 0.0,
                steer: 1.0,
            },
            3.0,
        );
        assert!(kinds.iter().any(|k| k == "WheelLift"), "events: {kinds:?}");
        assert!(kinds.iter().any(|k| k == "RolledOver"), "events: {kinds:?}");
    }

    #[test]
    fn a_normal_car_does_not_lift_wheels_on_the_limit() {
        let def = sandbox_vehicle("hatchback");
        let (mut app, e) = app_with(&def);
        crate::set_rolling_speed(&mut app.world, e, 60.0 / 3.6);
        let kinds = drive(
            &mut app,
            e,
            VehicleInput {
                throttle: 0.3,
                brake: 0.0,
                steer: 1.0,
            },
            3.0,
        );
        assert!(!kinds.iter().any(|k| k == "WheelLift"), "events: {kinds:?}");
    }

    #[test]
    fn pumping_air_brakes_drains_the_reservoir() {
        let mut def = sandbox_vehicle("city_bus");
        if let Some(air) = def.brakes.air.as_mut() {
            air.compressor_bar_per_s = 0.05;
        }
        let (mut app, e) = app_with(&def);
        let mut kinds = Vec::new();
        for _ in 0..25 {
            kinds.extend(drive(
                &mut app,
                e,
                VehicleInput {
                    brake: 1.0,
                    ..Default::default()
                },
                0.8,
            ));
            kinds.extend(drive(&mut app, e, VehicleInput::default(), 0.8));
        }
        assert!(
            kinds.iter().any(|k| k == "LowAirPressure"),
            "events: {kinds:?}"
        );
        let air = app.world.get::<crate::Vehicle>(e).unwrap().air.unwrap();
        assert!(air.reservoir_bar < 5.5, "reservoir {}", air.reservoir_bar);
    }
}
