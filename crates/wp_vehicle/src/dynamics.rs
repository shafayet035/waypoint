//! One physics substep of the vehicle model.

use crate::def::{Drive, VehicleDef};
use crate::{Vehicle, VehicleEvent, VehicleInput, VehicleLibrary, WHEEL_NAMES, parts};
use glam::Vec2;
use wp_core::SimTime;
use wp_ecs::{Events, World};
use wp_physics::{Footing, Gravity, Ground, Pose, RigidBody, Terrain, Velocity};

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
    let terrain = world.get_resource::<Terrain>().map(|t| t.0.clone());
    let water = world.get_resource::<wp_physics::Water>().map(|w| w.clone());
    // Crash damage that changes how a vehicle drives (power, steering pull).
    let mut harm: std::collections::BTreeMap<wp_ecs::Entity, Harm> = Default::default();
    world
        .query::<(&Vehicle, &crate::Damage)>()
        .for_each(|e, (veh, d)| {
            if let Some(def) = library.0.get(&veh.model) {
                harm.insert(e, Harm::from_damage(def, d));
            }
        });
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
                // On a slope only part of the weight presses into the ground;
                // the rest pulls the vehicle downhill. Over crests and dips
                // the ground's vertical acceleration lightens or loads it, and
                // in the air nothing presses at all.
                let (g_normal, pull) = match &terrain {
                    Some(field) => {
                        let f = footing(def, pose, field);
                        let speed = vel.linear.length();
                        let k = update_vertical(veh, &f, g, dt, speed, &e, &mut local_events);
                        if k == 0.0 {
                            (0.0, Vec2::ZERO)
                        } else {
                            (g * f.normal_share() * k, f.sines() * g)
                        }
                    }
                    None => (g, Vec2::ZERO),
                };
                let depth = match (&terrain, &water) {
                    (Some(field), Some(w)) => w.depth(field, pose.position),
                    _ => 0.0,
                };
                // Wet tyres in water grip less (up to 40 % less once the
                // wheels are half under).
                let surface = if depth > 0.0 {
                    surface * (1.0 - 0.4 * (depth / def.wheel.radius_m).min(1.0))
                } else {
                    surface
                };
                let ctx = Ctx {
                    def,
                    dt,
                    g: g_normal,
                    pull,
                    water_depth: depth,
                    surface,
                    entity: e.to_string(),
                    harm: harm.get(&e).copied().unwrap_or_default(),
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

/// Water ploughing against a vehicle wading `depth` m deep, body frame, N:
/// first the tyres (about 0.8 m² of them per metre of depth), then the body
/// once the water rises past its floor (0.25 m), sideways over its length.
fn water_drag(def: &VehicleDef, depth: f32, v_local: Vec2) -> Vec2 {
    const WATER_DENSITY: f32 = 1000.0;
    const DRAG_COEFFICIENT: f32 = 0.6;
    const FLOOR_M: f32 = 0.25;
    let (width, length) = def
        .body
        .as_ref()
        .map_or((def.track_width_m + 0.3, def.wheelbase_m + 1.6), |b| {
            (b.width_m, b.length_m)
        });
    let tyres = 0.8 * depth.min(FLOOR_M);
    let above = (depth - FLOOR_M).max(0.0);
    let front = tyres + width * above;
    let side = tyres * 2.0 + length * above;
    let q = 0.5 * WATER_DENSITY * DRAG_COEFFICIENT * v_local.length();
    Vec2::new(-q * front * v_local.x, -q * side * v_local.y)
}

/// Below this speed (m/s) and yaw rate (rad/s) a body counts as standing:
/// on a slope, static friction then holds it (see [`unheld`]).
const STANDING_MPS: f32 = 0.2;
const STANDING_YAW: f32 = 0.05;

pub(crate) fn standing(v_local: Vec2, yaw_rate: f32) -> bool {
    v_local.length() < STANDING_MPS && yaw_rate.abs() < STANDING_YAW
}

/// Static friction on a slope. The tyre model makes grip from sliding, so
/// alone it would let a braked vehicle creep downhill; a standing body's
/// tyres instead hold gravity's pull (`gravity`, N, body frame) up to what
/// they can (`hold`: along, across, N). Returns the part they do not hold.
pub(crate) fn unheld(gravity: Vec2, hold: Vec2) -> Vec2 {
    let part = |f: f32, h: f32| f.signum() * (f.abs() - h).max(0.0);
    Vec2::new(part(gravity.x, hold.x), part(gravity.y, hold.y))
}

/// Time constant of the filter on the ground's vertical acceleration, s
/// (the grid's bilinear cells have kinks the suspension would smooth).
const VERTICAL_FILTER_S: f32 = 0.1;
/// Most extra load the ground's push can add (× weight).
const MAX_LOAD_FACTOR: f32 = 3.0;
/// Shortest time in the air that counts as a jump (events), s.
const MIN_JUMP_S: f32 = 0.1;

/// Vertical motion over the ground under the centre (`ground`, m): follow
/// it, or fly and land. Returns the load factor: 1 on level ground, more in
/// dips and on landing, less over crests, 0 in the air.
fn update_vertical(
    veh: &mut Vehicle,
    footing: &Footing,
    g: f32,
    dt: f32,
    speed: f32,
    entity: &wp_ecs::Entity,
    events: &mut Vec<VehicleEvent>,
) -> f32 {
    let ground = footing.contacts_m;
    let v = veh.vertical.get_or_insert(crate::Vertical {
        z_m: ground,
        ground_m: ground,
        ..Default::default()
    });
    // Placed somewhere new (a reset or a teleport): start on the ground.
    if (ground - v.ground_m).abs() > 2.0 && !v.airborne {
        *v = crate::Vertical {
            z_m: ground,
            ground_m: ground,
            ..Default::default()
        };
    }
    let ground_vz = (ground - v.ground_m) / dt;
    v.ground_m = ground;
    if v.airborne {
        let was = v.air_s;
        v.air_s += dt;
        v.vz -= g * dt;
        v.z_m += v.vz * dt;
        // Report a jump once it is one (not a skip of a few milliseconds).
        if was < MIN_JUMP_S && v.air_s >= MIN_JUMP_S {
            events.push(VehicleEvent::Airborne {
                entity: entity.to_string(),
                speed_kmh: speed * 3.6,
            });
        }
        if v.z_m > ground {
            return 0.0;
        }
        // Landing: the suspension takes the closing speed over about 0.12 s.
        let impact = (ground_vz - v.vz).max(0.0);
        if v.air_s >= MIN_JUMP_S {
            events.push(VehicleEvent::Landed {
                entity: entity.to_string(),
                air_s: v.air_s,
                impact_ms: impact,
            });
        }
        *v = crate::Vertical {
            z_m: ground,
            vz: ground_vz,
            ground_m: ground,
            accel: (impact / 0.12).min((MAX_LOAD_FACTOR - 1.0) * g),
            airborne: false,
            air_s: 0.0,
            grade: footing.grade.to_array(),
        };
        return (1.0 + v.accel / g).clamp(0.0, MAX_LOAD_FACTOR);
    }
    let raw = (ground_vz - v.vz) / dt;
    v.accel += (raw - v.accel) * dt / (VERTICAL_FILTER_S + dt);
    v.vz = ground_vz;
    v.z_m = ground;
    v.grade = footing.grade.to_array();
    let factor = 1.0 + v.accel / g;
    if factor < 0.0 && speed > 5.0 {
        // The crest falls away faster than gravity can pull it down.
        v.airborne = true;
        v.air_s = 0.0;
        return 0.0;
    }
    factor.clamp(0.0, MAX_LOAD_FACTOR)
}

/// How a vehicle stands on `field`: contact points at its wheels.
pub fn footing(def: &VehicleDef, pose: &Pose, field: &wp_physics::HeightField) -> Footing {
    Footing::on(
        field,
        pose.position,
        pose.heading,
        def.cg_to_front_axle_m,
        def.rear_axle_from_cg(),
        def.track_width_m / 2.0,
    )
}

/// One tyre's contact for this substep: sliding friction, relaxed slip states,
/// brush force and low-speed damping. Updates the slip state in `w` and returns
/// `(fx, fy, kinematic longitudinal slip)` in the wheel frame. Shared by
/// vehicles and trailers.
pub(crate) fn tyre_contact(
    w: &mut crate::WheelState,
    vt: Vec2,
    load: f32,
    peak_mu: f32,
    tyre: &crate::def::TyreDef,
    radius: f32,
    dt: f32,
) -> (f32, f32, f32) {
    let k_tyre = tyre.stiffness_per_load;
    // Sliding friction from the kinematic (instantaneous) slip.
    let ref_speed = vt.x.abs().max(1.0);
    let kin_long = (w.omega * radius - vt.x) / ref_speed;
    let kin_lat = vt.y / ref_speed;
    let slide = (kin_long * kin_long + kin_lat * kin_lat).sqrt();
    let mu = parts::sliding_friction(peak_mu, tyre.sliding_friction_ratio, k_tyre, slide);

    // Relaxed slip states; the relaxation length shrinks near standstill so
    // little carcass wind-up is stored when the vehicle stops.
    let relax_eff =
        tyre.relaxation_length_m * (vt.x.abs() / LOW_SPEED_DAMPING_BELOW).clamp(0.1, 1.0);
    let damping_speed = vt.x.abs() + SLIP_DAMPING_SPEED;
    w.slip_ratio += ((w.omega * radius - vt.x) - damping_speed * w.slip_ratio) * dt / relax_eff;
    w.slip_tan += (vt.y - damping_speed * w.slip_tan) * dt / relax_eff;
    (w.slip_ratio, w.slip_tan) = parts::cap_slip(mu, k_tyre, w.slip_ratio, w.slip_tan);

    let (mut fx, mut fy) = parts::brush_tyre(load, mu, k_tyre, w.slip_ratio, w.slip_tan);
    // Near standstill, damp contact-patch sliding so a stopped vehicle settles.
    let fade = (1.0 - vt.x.abs() / LOW_SPEED_DAMPING_BELOW).max(0.0);
    if fade > 0.0 {
        let c = CONTACT_DAMPING_PER_LOAD * load * fade;
        fx += c * (w.omega * radius - vt.x);
        fy -= c * vt.y;
        let limit = mu * load;
        let f = (fx * fx + fy * fy).sqrt();
        if f > limit {
            fx *= limit / f;
            fy *= limit / f;
        }
    }
    (fx, fy, kin_long)
}

/// Everything a phase needs besides the vehicle's own state.
struct Ctx<'a> {
    def: &'a VehicleDef,
    dt: f32,
    /// Gravity pressing into the ground (all of it on the flat), m/s².
    g: f32,
    /// Gravity along the ground in the body frame, uphill positive (zero
    /// on the flat), m/s²: forward for nose-up, left for left-side-up.
    pull: Vec2,
    /// Water depth where it stands, m (0 on dry land).
    water_depth: f32,
    surface: f32,
    entity: String,
    harm: Harm,
}

/// How crash damage changes driving.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Harm {
    /// Share of engine power left (damaged cooling, intake, drivetrain).
    pub power: f32,
    /// Steady steering pull from bent front suspension, rad (left positive).
    pub pull_rad: f32,
}

impl Default for Harm {
    fn default() -> Self {
        Harm {
            power: 1.0,
            pull_rad: 0.0,
        }
    }
}

impl Harm {
    /// Full power above 70 % health, falling to 35 % when wrecked; a dent on
    /// one front corner pulls the steering towards it (about 1° per 30 cm).
    pub fn from_damage(def: &VehicleDef, d: &crate::Damage) -> Harm {
        let h = d.health_pct.clamp(0.0, 100.0);
        let power = if h >= 70.0 {
            1.0
        } else {
            0.35 + 0.65 * h / 70.0
        };
        let half_track = def.track_width_m / 2.0;
        let pull_rad = d
            .dents
            .iter()
            .filter(|dent| dent.x > def.cg_to_front_axle_m - 0.6 && dent.y.abs() > half_track * 0.4)
            .map(|dent| dent.depth_m * 0.06 * dent.y.signum())
            .sum::<f32>()
            .clamp(-0.05, 0.05);
        Harm { power, pull_rad }
    }
}

/// The body roll model, needed again for load transfer and tipping.
struct RollModel {
    sprung_mass: f32,
    unsprung_mass: f32,
    stiffness: f32,
    damping: f32,
    inertia: f32,
}

/// What the drivetrain delivers this substep.
struct Drivetrain {
    driven: [bool; 4],
    wheel_torque: f32,
}

/// One substep for one vehicle: each phase reads the state the previous ones
/// left, in this order (the order is part of the simulation's determinism).
fn step_vehicle(
    ctx: &Ctx,
    veh: &mut Vehicle,
    input: &VehicleInput,
    pose: &Pose,
    vel: &Velocity,
    body: &mut RigidBody,
    events: &mut Vec<VehicleEvent>,
) {
    update_steering(ctx, veh, input);
    let v_local = pose.to_local(vel.linear);
    if ctx.water_depth > ctx.def.wading_depth() && !veh.flooded {
        veh.flooded = true;
        events.push(VehicleEvent::EngineFlooded {
            entity: ctx.entity.clone(),
            depth_m: ctx.water_depth,
        });
    }
    let (ax, ay) = measure_acceleration(ctx, veh, pose, vel);
    // The springs feel acceleration plus gravity's pull along a slope (a car
    // parked facing uphill sits back on its rear axle).
    let (ax, ay) = if ctx.pull == Vec2::ZERO {
        (ax, ay)
    } else {
        (ax + ctx.pull.x, ay + ctx.pull.y)
    };
    let roll = update_body_motion(ctx, veh, ax, ay);
    let loads = if veh.vertical.is_some_and(|v| v.airborne) {
        [0.0; 4]
    } else {
        wheel_loads(ctx, veh, &roll, ax, ay, events)
    };
    let esc_torque = stability_control(ctx, veh, v_local, vel.angular, events);
    traction_control(ctx, veh, input, v_local);
    let drive = update_drivetrain(ctx, veh, input, v_local, events);
    let mut brake_torque = update_brakes(ctx, veh, input, events);
    for (b, e) in brake_torque.iter_mut().zip(esc_torque) {
        *b += e;
    }
    let (force, moment) = tyre_forces(
        ctx,
        veh,
        v_local,
        vel.angular,
        &loads,
        &drive,
        &brake_torque,
        events,
    );
    let (mut force, moment) = add_body_forces(ctx, veh, v_local, vel.angular, force, moment);
    if ctx.water_depth > 0.0 {
        force += water_drag(ctx.def, ctx.water_depth, v_local);
    }
    if ctx.pull != Vec2::ZERO {
        let def = ctx.def;
        let mut gravity = -ctx.pull * def.mass_kg;
        if standing(v_local, vel.angular) {
            // Braked (and rolling) tyres hold it along; any tyre holds it across.
            let peak = def.tyre.friction * ctx.surface;
            let along: f32 = (0..4)
                .map(|i| {
                    (brake_torque[i] / def.wheel.radius_m + def.tyre.rolling_resistance * loads[i])
                        .min(peak * loads[i])
                })
                .sum();
            let across = peak * loads.iter().sum::<f32>();
            gravity = unheld(gravity, Vec2::new(along, across));
        }
        force += gravity;
    }
    body.apply_force(pose.to_world(force));
    body.apply_torque(moment);
}

/// Rate-limited steering towards the driver's input.
fn update_steering(ctx: &Ctx, veh: &mut Vehicle, input: &VehicleInput) {
    let def = ctx.def;
    let target =
        input.steer.clamp(-1.0, 1.0) * def.steering.max_angle_deg.to_radians() + ctx.harm.pull_rad;
    let max_step = def.steering.rate_deg_per_s.to_radians() * ctx.dt;
    veh.steer_angle += (target - veh.steer_angle).clamp(-max_step, max_step);
}

/// Acceleration measured from the velocity change over the last substep: it
/// includes everything that moved the body (tyres, a trailer's pull, joints).
/// Returns the filtered body-frame (ax, ay).
fn measure_acceleration(ctx: &Ctx, veh: &mut Vehicle, pose: &Pose, vel: &Velocity) -> (f32, f32) {
    let dt = ctx.dt;
    let a_world = (vel.linear - veh.prev_velocity) / dt;
    veh.prev_velocity = vel.linear;
    let alpha = dt / (ACCEL_FILTER_S + dt);
    veh.accel_local += (pose.to_local(a_world) - veh.accel_local) * alpha;
    (veh.accel_local.x, veh.accel_local.y)
}

/// Body roll and pitch as damped springs about the roll and pitch axes.
fn update_body_motion(ctx: &Ctx, veh: &mut Vehicle, ax: f32, ay: f32) -> RollModel {
    let (def, dt, g) = (ctx.def, ctx.dt, ctx.g);
    let (m, h, len, track, radius) = (
        def.mass_kg,
        def.cg_height_m,
        def.wheelbase_m,
        def.track_width_m,
        def.wheel.radius_m,
    );
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
    RollModel {
        sprung_mass: ms,
        unsprung_mass: mu_mass,
        stiffness: k_roll,
        damping: c_roll,
        inertia: i_roll,
    }
}

/// Vertical load on each wheel (FL, FR, RL, RR): static share, longitudinal and
/// lateral transfer, a coupled trailer's kingpin load, and tipping about the
/// outer wheels. Sends WheelLift/WheelLanded/RolledOver events.
fn wheel_loads(
    ctx: &Ctx,
    veh: &mut Vehicle,
    roll: &RollModel,
    ax: f32,
    ay: f32,
    events: &mut Vec<VehicleEvent>,
) -> [f32; 4] {
    let (def, dt, g) = (ctx.def, ctx.dt, ctx.g);
    let (m, h, len, track, radius) = (
        def.mass_kg,
        def.cg_height_m,
        def.wheelbase_m,
        def.track_width_m,
        def.wheel.radius_m,
    );
    let b = def.rear_axle_from_cg();
    let susp = &def.suspension;
    let front_axle = (m * g * b / len - m * ax * h / len).max(0.0);
    let rear_axle = (m * g - front_axle).max(0.0);
    // A semi-trailer's kingpin load sits on the fifth wheel, between the axles.
    let (front_axle, rear_axle) = match &def.hitch {
        Some(hitch) => {
            let fk = veh.hitch_load_n.max(0.0);
            let to_front = fk * hitch.ahead_of_rear_axle_m / len;
            (front_axle + to_front, rear_axle + fk - to_front)
        }
        None => (front_axle, rear_axle),
    };
    // Lateral transfer: through the springs (from roll) and through the roll
    // centre and unsprung mass (instant). Positive moves load to the right.
    let elastic = roll.stiffness * veh.roll + roll.damping * veh.roll_rate;
    let geometric =
        roll.sprung_mass * ay * susp.roll_center_height_m + roll.unsprung_mass * ay * radius;
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
        let i_tip = roll.inertia + m * (h * h + half * half);
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
    loads
}

/// Engine, automatic gearbox (shifting on road speed) and open differential.
/// Returns which wheels are driven and the torque each receives.
fn update_drivetrain(
    ctx: &Ctx,
    veh: &mut Vehicle,
    input: &VehicleInput,
    v_local: Vec2,
    events: &mut Vec<VehicleEvent>,
) -> Drivetrain {
    let (def, dt) = (ctx.def, ctx.dt);
    let radius = def.wheel.radius_m;
    let driven = match def.drive {
        Drive::Front => [true, true, false, false],
        Drive::Rear => [false, false, true, true],
        Drive::All => [true; 4],
    };
    let n_driven = driven.iter().filter(|d| **d).count() as f32;
    let gears = &def.gearbox.ratios;
    veh.gear = veh.gear.clamp(1, gears.len().max(1) as u8);
    // Reverse engages (or disengages) only when nearly stopped.
    if v_local.length() < 0.55 {
        veh.reverse = input.reverse;
    }
    let ratio = if veh.reverse {
        -def.gearbox
            .reverse_ratio
            .unwrap_or_else(|| gears.first().copied().unwrap_or(1.0))
            * def.gearbox.final_drive
    } else {
        gears.get(veh.gear as usize - 1).copied().unwrap_or(1.0) * def.gearbox.final_drive
    };
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
    let engine_torque = if rpm_from_wheels >= engine.redline_rpm || veh.flooded {
        0.0
    } else if throttle > 0.02 {
        parts::curve_torque(&engine.torque_curve, veh.engine_rpm)
            * throttle
            * veh.tc_factor
            * ctx.harm.power
    } else if rpm_from_wheels > engine.idle_rpm {
        -engine.engine_brake_nm * (rpm_from_wheels / engine.redline_rpm)
    } else {
        0.0
    };
    let wheel_torque = engine_torque * ratio * def.gearbox.efficiency / n_driven.max(1.0);

    // Shift on road speed, not wheel speed, so wheelspin cannot cause hunting.
    let road_rpm = v_local.x.max(0.0) / radius * ratio * 60.0 / std::f32::consts::TAU;
    veh.shift_timer = (veh.shift_timer - dt).max(0.0);
    if veh.shift_timer == 0.0 && !veh.reverse {
        let from = veh.gear;
        if road_rpm > def.gearbox.shift_up_rpm
            && (veh.gear as usize) < gears.len()
            && can_hold_next_gear(ctx, veh, ratio, engine_torque, road_rpm, throttle)
        {
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
    Drivetrain {
        driven,
        wheel_torque,
    }
}

/// Climbing, an automatic gearbox only shifts up if the next gear still
/// pulls harder than the hill and the air hold the vehicle back; otherwise
/// it would drop back at once and hunt between the two. On level ground
/// (or downhill) it always shifts.
fn can_hold_next_gear(
    ctx: &Ctx,
    veh: &Vehicle,
    ratio: f32,
    engine_torque: f32,
    road_rpm: f32,
    throttle: f32,
) -> bool {
    if ctx.pull.x <= 0.0 || throttle < 0.5 {
        return true;
    }
    let def = ctx.def;
    let (radius, eff) = (def.wheel.radius_m, def.gearbox.efficiency);
    let next = def.gearbox.ratios[veh.gear as usize] * def.gearbox.final_drive;
    let pull_now = engine_torque * ratio * eff / radius;
    // What holds it back: the pull it has now less what goes into speeding up.
    let resistance = pull_now - def.mass_kg * veh.accel_local.x;
    let rpm_next = road_rpm * next / ratio;
    let torque_next = parts::curve_torque(&def.engine.torque_curve, rpm_next)
        * throttle
        * veh.tc_factor
        * ctx.harm.power;
    torque_next * next * eff / radius > resistance * 1.05
}

/// Driven-wheel slip above which traction control cuts torque.
const TC_SLIP: f32 = 0.12;
/// How fast traction control gives torque back, per second.
const TC_RECOVERY_PER_S: f32 = 1.5;

/// Traction control: cut engine torque at once when a driven wheel spins,
/// give it back gradually. Stability control may also hold torque down.
fn traction_control(ctx: &Ctx, veh: &mut Vehicle, input: &VehicleInput, v_local: Vec2) {
    let def = ctx.def;
    let mut limit: f32 = 1.0;
    if def.brakes.traction_control && input.throttle > 0.02 && !veh.reverse {
        let driven = match def.drive {
            Drive::Front => [true, true, false, false],
            Drive::Rear => [false, false, true, true],
            Drive::All => [true; 4],
        };
        let road = v_local.x.max(0.0);
        let slip = veh
            .wheels
            .iter()
            .zip(driven)
            .filter(|(_, d)| *d)
            .map(|(w, _)| (w.omega * def.wheel.radius_m - road) / road.max(2.0))
            .fold(0.0f32, f32::max);
        if slip > TC_SLIP {
            limit = (1.0 - 4.0 * (slip - TC_SLIP)).clamp(0.15, 1.0);
        }
    }
    if veh.esc_active {
        limit = limit.min(0.3);
    }
    veh.tc_factor = if limit < veh.tc_factor {
        limit
    } else {
        (veh.tc_factor + TC_RECOVERY_PER_S * ctx.dt).min(limit)
    };
    veh.tc_active = def.brakes.traction_control && veh.tc_factor < 0.999;
}

/// Stability control: the yaw rate the steering asks for (a neutral-steering
/// bicycle model, capped at what the tyres can give) against the real one.
/// Too much yaw (a slide) brakes the outer front wheel; too little (a
/// plough) brakes the inner rear. Returns extra brake torque per wheel.
fn stability_control(
    ctx: &Ctx,
    veh: &mut Vehicle,
    v_local: Vec2,
    yaw_rate: f32,
    events: &mut Vec<VehicleEvent>,
) -> [f32; 4] {
    let def = ctx.def;
    let mut out = [0.0; 4];
    let vx = v_local.x;
    let active = if def.brakes.esc && vx > 5.0 && !veh.reverse && !veh.rolled_over {
        let wanted = vx * veh.steer_angle / def.wheelbase_m;
        let cap = 0.85 * def.tyre.friction * ctx.surface * ctx.g / vx;
        let wanted = wanted.clamp(-cap, cap);
        // A growing sideslip angle counts as extra yaw: it is the slide
        // starting before the yaw rate shows it.
        let beta = v_local.y.atan2(vx);
        let slide = (beta.abs() - 0.05).max(0.0) * -beta.signum() * 1.5;
        let error = yaw_rate - wanted + slide;
        let threshold = 0.05 + 0.08 * wanted.abs();
        // Like real systems, stay out of calm driving: act only while the
        // car corners hard or is already sliding.
        let working = veh.accel_local.y.abs() > 0.25 * ctx.g || beta.abs() > 3f32.to_radians();
        if working && error.abs() > threshold {
            // Braking a left wheel turns the car left (positive yaw).
            let left = error < 0.0;
            let sliding = yaw_rate.abs() > wanted.abs() || yaw_rate * wanted < 0.0;
            let wheel = match (left, sliding) {
                (true, true) => 0,   // front left
                (false, true) => 1,  // front right
                (true, false) => 2,  // rear left
                (false, false) => 3, // rear right
            };
            let gain = def.brakes.max_torque_nm * 2.5;
            out[wheel] = (gain * (error.abs() - threshold)).min(def.brakes.max_torque_nm * 0.6);
            true
        } else {
            false
        }
    } else {
        false
    };
    if active != veh.esc_active {
        veh.esc_active = active;
        events.push(if active {
            VehicleEvent::EscEngaged {
                entity: ctx.entity.clone(),
            }
        } else {
            VehicleEvent::EscReleased {
                entity: ctx.entity.clone(),
            }
        });
    }
    out
}

/// Pedal → (air system) → brake torque per wheel, before ABS.
fn update_brakes(
    ctx: &Ctx,
    veh: &mut Vehicle,
    input: &VehicleInput,
    events: &mut Vec<VehicleEvent>,
) -> [f32; 4] {
    let def = ctx.def;
    let pedal = input.brake.clamp(0.0, 1.0);
    let brake_fraction = match (&def.brakes.air, veh.air.as_mut()) {
        (Some(air), Some(state)) => {
            let was_ok = state.reservoir_bar >= air.warning_bar;
            let f = parts::air_brake_step(state, air, pedal, ctx.dt);
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
    veh.brake_fraction = brake_fraction;
    let brake = brake_fraction * def.brakes.max_torque_nm;
    let front_brake = brake * def.brakes.front_bias / 2.0;
    let rear_brake = brake * (1.0 - def.brakes.front_bias) / 2.0;
    [front_brake, front_brake, rear_brake, rear_brake]
}

/// Every tyre: slip, force, ABS valve and wheel spin. Returns the summed
/// body-frame force and yaw moment, and sends AbsEngaged/AbsReleased.
#[allow(clippy::too_many_arguments)]
fn tyre_forces(
    ctx: &Ctx,
    veh: &mut Vehicle,
    v_local: Vec2,
    yaw_rate: f32,
    loads: &[f32; 4],
    drive: &Drivetrain,
    brake_torque: &[f32; 4],
    events: &mut Vec<VehicleEvent>,
) -> (Vec2, f32) {
    let (def, dt) = (ctx.def, ctx.dt);
    let (a, b) = (def.cg_to_front_axle_m, def.rear_axle_from_cg());
    let radius = def.wheel.radius_m;
    let peak_mu = def.tyre.friction * ctx.surface;
    let half_track = def.track_width_m / 2.0;
    let positions = [
        Vec2::new(a, half_track),
        Vec2::new(a, -half_track),
        Vec2::new(-b, half_track),
        Vec2::new(-b, -half_track),
    ];
    let steer = [veh.steer_angle, veh.steer_angle, 0.0, 0.0];
    let speed = v_local.length();
    let mut force = Vec2::ZERO;
    let mut moment = 0.0;
    let mut abs_any = false;
    for i in 0..4 {
        let p = positions[i];
        let contact_vel = v_local + Vec2::new(-yaw_rate * p.y, yaw_rate * p.x);
        let wheel_rot = Vec2::from_angle(steer[i]);
        let vt = Vec2::from_angle(-steer[i]).rotate(contact_vel);
        let w = &mut veh.wheels[i];

        let (fx, fy, kin_long) = tyre_contact(w, vt, loads[i], peak_mu, &def.tyre, radius, dt);

        if def.brakes.abs {
            w.abs_factor =
                parts::abs_step(w.abs_factor, kin_long, brake_torque[i] > 0.0, speed, dt);
            abs_any |= w.abs_factor < 0.999;
        } else {
            w.abs_factor = 1.0;
        }
        let torque = if drive.driven[i] {
            drive.wheel_torque
        } else {
            0.0
        };
        let resist =
            brake_torque[i] * w.abs_factor + def.tyre.rolling_resistance * loads[i] * radius;
        w.omega = parts::step_wheel(
            w.omega,
            torque,
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
    (force, moment)
}

/// Forces on the body itself: scraping along the ground once rolled over, and
/// aerodynamic drag.
fn add_body_forces(
    ctx: &Ctx,
    veh: &Vehicle,
    v_local: Vec2,
    yaw_rate: f32,
    mut force: Vec2,
    mut moment: f32,
) -> (Vec2, f32) {
    let (def, g) = (ctx.def, ctx.g);
    let (m, len) = (def.mass_kg, def.wheelbase_m);
    if veh.rolled_over {
        let speed = v_local.length();
        const BODY_FRICTION: f32 = 0.5;
        force += -BODY_FRICTION * m * g * v_local / speed.max(0.5);
        moment +=
            -BODY_FRICTION * m * g * yaw_rate * len / (yaw_rate.abs() * len).max(0.5) * len / 4.0;
    }
    force += -0.5 * AIR_DENSITY * def.aero.drag_area_m2 * v_local.length() * v_local;
    (force, moment)
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

    /// A 10 % slope rising eastward over the whole test area.
    fn on_ramp(app: &mut App) {
        let mut f = wp_physics::HeightField::flat(Vec2::new(-500.0, -500.0), 10.0, 101, 101);
        for j in 0..101 {
            for i in 0..101 {
                f.heights[j * 101 + i] = 50.0 + i as f32;
            }
        }
        app.world
            .insert_resource(wp_physics::Terrain(std::sync::Arc::new(f)));
    }

    fn speed_along(app: &App, e: Entity) -> f32 {
        app.world.get::<wp_physics::Velocity>(e).unwrap().linear.x
    }

    #[test]
    fn slopes_pull_downhill_and_brakes_hold() {
        let def = sandbox_vehicle("hatchback");
        let idle = VehicleInput::default();
        // Facing uphill with nothing pressed: rolls back.
        let (mut app, e) = app_with(&def);
        on_ramp(&mut app);
        drive(&mut app, e, idle, 3.0);
        let back = speed_along(&app, e);
        assert!(back < -1.5, "should roll back: {back}");
        // On the flat it stays put.
        let (mut flat, f) = app_with(&def);
        drive(&mut flat, f, idle, 3.0);
        assert!(speed_along(&flat, f).abs() < 0.05);
        // Brakes hold it on the slope.
        let (mut app, e) = app_with(&def);
        on_ramp(&mut app);
        let held = VehicleInput { brake: 0.6, ..idle };
        drive(&mut app, e, held, 3.0);
        assert!(
            speed_along(&app, e).abs() < 0.05,
            "{}",
            speed_along(&app, e)
        );
        // ...and keeps holding it: no creeping downhill.
        let x0 = app.world.get::<wp_physics::Pose>(e).unwrap().position;
        drive(&mut app, e, held, 30.0);
        let x1 = app.world.get::<wp_physics::Pose>(e).unwrap().position;
        assert!(
            x0.distance(x1) < 0.005,
            "crept {} m in 30 s",
            x0.distance(x1)
        );
        // Weight moves onto the rear wheels facing uphill (about m·g·sinθ·h/L).
        let (mut flat, f) = app_with(&def);
        drive(&mut flat, f, held, 3.0);
        let rear_share = |app: &App, e: Entity| {
            let veh = app.world.get::<crate::Vehicle>(e).unwrap();
            veh.wheels[2].load / (veh.wheels[0].load + veh.wheels[2].load)
        };
        let (up, level) = (rear_share(&app, e), rear_share(&flat, f));
        let expected = 0.1 * def.cg_height_m / def.wheelbase_m;
        assert!(
            (up - level - expected).abs() < 0.01,
            "rear share {up} vs {level} on the flat, expected +{expected}"
        );
        // Full throttle climbs, but slower than on the flat.
        let go = VehicleInput {
            throttle: 1.0,
            ..idle
        };
        let (mut app, e) = app_with(&def);
        on_ramp(&mut app);
        drive(&mut app, e, go, 6.0);
        let (mut flat, f) = app_with(&def);
        drive(&mut flat, f, go, 6.0);
        let (up, level) = (speed_along(&app, e), speed_along(&flat, f));
        assert!(up > 5.0 && up < level * 0.92, "uphill {up} flat {level}");
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
                reverse: false,
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
                reverse: false,
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

    /// A sine-with-dwell style manoeuvre (FMVSS 126) at 80 km/h, raw inputs:
    /// returns the worst sideslip (deg), the peak yaw rate and the yaw rate
    /// 1 s after the steering ends (deg/s), and the events seen.
    fn sine_with_dwell(esc: bool) -> (f32, f32, f32, Vec<String>) {
        let mut def = sandbox_vehicle("hatchback");
        def.brakes.esc = esc;
        let (mut app, e) = app_with(&def);
        crate::set_rolling_speed(&mut app.world, e, 80.0 / 3.6);
        let mut kinds = Vec::new();
        let (mut worst, mut peak, mut after) = (0.0f32, 0.0f32, 0.0f32);
        let mut t = 0.0f32;
        // 0.7 Hz sine, dwelling 0.5 s at the second peak, then straight.
        let steer_at = |t: f32| {
            let w = std::f32::consts::TAU * 0.7;
            let amp = 0.45;
            let quarter3 = 0.75 / 0.7;
            if t < quarter3 {
                amp * (w * t).sin()
            } else if t < quarter3 + 0.5 {
                -amp
            } else if t < 1.0 / 0.7 + 0.5 {
                amp * (w * (t - 0.5)).sin()
            } else {
                0.0
            }
        };
        let end = 1.0 / 0.7 + 0.5;
        while t < end + 2.0 {
            kinds.extend(drive(
                &mut app,
                e,
                VehicleInput {
                    throttle: 0.0,
                    brake: 0.0,
                    steer: steer_at(t),
                    reverse: false,
                },
                1.0 / 60.0,
            ));
            t += 1.0 / 60.0;
            let tel = crate::telemetry(&app.world, e).unwrap();
            worst = worst.max(tel.sideslip_deg.abs());
            peak = peak.max(tel.yaw_rate_deg_s.abs());
            if (t - (end + 1.0)).abs() < 0.5 / 60.0 {
                after = tel.yaw_rate_deg_s.abs();
            }
        }
        (worst, peak, after, kinds)
    }

    #[test]
    fn stability_control_passes_sine_with_dwell() {
        let (raw, raw_peak, raw_after, raw_events) = sine_with_dwell(false);
        let (esc, peak, after, esc_events) = sine_with_dwell(true);
        eprintln!(
            "without ESC: sideslip {raw:.1}°, yaw {raw_after:.1} of {raw_peak:.1} °/s 1 s after; \
             with: sideslip {esc:.1}°, yaw {after:.1} of {peak:.1} °/s"
        );
        assert!(!raw_events.iter().any(|k| k == "EscEngaged"));
        assert!(
            esc_events.iter().any(|k| k == "EscEngaged"),
            "{esc_events:?}"
        );
        // FMVSS 126: yaw rate 1 s after the steering ends below 35 % of its peak.
        assert!(after < 0.35 * peak, "ESC: {after:.1} of {peak:.1} °/s");
        assert!(
            esc < raw,
            "ESC should reduce the slide: {esc:.1}° vs {raw:.1}°"
        );
    }

    #[test]
    fn traction_control_stops_wheelspin() {
        let launch = |tc: bool| {
            let mut def = sandbox_vehicle("hatchback");
            def.brakes.traction_control = tc;
            def.tyre.friction = 0.35; // ice-like
            let (mut app, e) = app_with(&def);
            let mut worst_slip = 0.0f32;
            for _ in 0..120 {
                drive(
                    &mut app,
                    e,
                    VehicleInput {
                        throttle: 1.0,
                        ..Default::default()
                    },
                    1.0 / 60.0,
                );
                let v = app.world.get::<crate::Vehicle>(e).unwrap();
                let road = app
                    .world
                    .get::<wp_physics::Velocity>(e)
                    .unwrap()
                    .linear
                    .length();
                let spin = v.wheels[0].omega * def.wheel.radius_m - road;
                worst_slip = worst_slip.max(spin);
            }
            let speed = crate::telemetry(&app.world, e).unwrap().speed_kmh;
            (worst_slip, speed)
        };
        let (raw_spin, raw_kmh) = launch(false);
        let (tc_spin, tc_kmh) = launch(true);
        eprintln!(
            "wheelspin {raw_spin:.1} m/s → {tc_spin:.1} m/s; speed after 2 s {raw_kmh:.1} → {tc_kmh:.1} km/h"
        );
        assert!(tc_spin < raw_spin * 0.5, "{tc_spin} vs {raw_spin}");
        assert!(
            tc_kmh >= raw_kmh * 0.95,
            "TC should not cost acceleration: {tc_kmh} vs {raw_kmh}"
        );
    }

    #[test]
    fn crash_damage_costs_power_and_pulls_the_steering() {
        let def = sandbox_vehicle("hatchback");
        let run = |damage: Option<crate::Damage>| {
            let (mut app, e) = app_with(&def);
            if let Some(d) = damage {
                app.world.insert(e, d);
            }
            drive(
                &mut app,
                e,
                VehicleInput {
                    throttle: 1.0,
                    ..Default::default()
                },
                6.0,
            );
            let t = crate::telemetry(&app.world, e).unwrap();
            (t.speed_kmh, t.position[1])
        };
        let (fresh_kmh, fresh_y) = run(None);
        // A hard hit on the front left corner.
        let wreck = crate::Damage {
            dents: vec![crate::Dent {
                x: 1.6,
                y: 0.8,
                depth_m: 0.4,
            }],
            health_pct: 30.0,
        };
        let (kmh, y) = run(Some(wreck));
        assert!(
            kmh < fresh_kmh * 0.8,
            "less power: {kmh:.1} vs {fresh_kmh:.1} km/h"
        );
        assert!(y > fresh_y + 0.5, "pulls left: {y:.2} m vs {fresh_y:.2} m");
        let light = crate::Damage {
            dents: vec![crate::Dent {
                x: -2.0,
                y: 0.0,
                depth_m: 0.05,
            }],
            health_pct: 93.0,
        };
        assert_eq!(
            super::Harm::from_damage(&def, &light),
            super::Harm::default(),
            "a rear scrape changes nothing"
        );
    }
}
