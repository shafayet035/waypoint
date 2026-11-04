//! AI traffic. NPC vehicles drive the same vehicle model as the player:
//! they pick random destinations and plan routes on the lane graph, follow
//! their lane (Stanley steering), keep their distance to whatever is ahead
//! (IDM, the player included), slow for bends and speed limits, and take
//! turns at junctions: a car claims its path across a junction before
//! entering and waits at the stop line while a crossing path is claimed or
//! its exit is blocked. Cars that reach a dead end at the map edge, or stay
//! stuck, reappear elsewhere.
//!
//! Set up with [`setup`] after the scene is applied; [`TrafficPlugin`]
//! spawns the vehicles on the first tick.

pub mod control;
pub mod idm;

use control::{TRACK_AHEAD_M, TRACK_BACK_M, route_speed_kmh, speed_control, stanley_steering};
use glam::Vec2;
use idm::Idm;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use wp_app::{App, Plugin};
use wp_core::SimRng;
use wp_ecs::Stage;
use wp_ecs::{Component, Entity, World};
use wp_physics::{Pose, RigidBody, Velocity};
use wp_road::{LaneId, LaneKind, Polyline, RoadNetwork};
use wp_vehicle::{
    Trailer, TrailerLibrary, Vehicle, VehicleDef, VehicleInput, VehicleLibrary, vehicle_footprint,
};
use wp_world::{Player, Roads, TrafficDef};

/// Marks an AI vehicle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrafficAgent {
    /// Destinations reached.
    pub trips: u32,
    /// Fraction of the speed limit this driver aims for.
    pub pace: f32,
}

impl Component for TrafficAgent {
    const NAME: &'static str = "TrafficAgent";
}

/// How traffic is doing, updated every tick (`world.resources` shows it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrafficStats {
    pub vehicles: u32,
    /// Destinations reached by all AI vehicles.
    pub trips: u32,
    /// Vehicles moved elsewhere after a dead end or being stuck.
    pub respawns: u32,
    /// Vehicles waiting at a junction's stop line right now.
    pub waiting: u32,
    /// Mean speed of the AI vehicles right now, km/h.
    pub mean_kmh: f32,
}

/// What to spawn, from the scene (see [`setup`]).
#[derive(Clone, Debug)]
pub struct TrafficSetup {
    pub def: TrafficDef,
}

/// Put traffic in a world whose scene has been applied: the vehicles it may
/// use are added to the vehicle library, and the plugin spawns them on the
/// first tick.
pub fn setup(world: &mut World, def: &TrafficDef, vehicles: &[&VehicleDef]) {
    if !world.has_resource::<VehicleLibrary>() {
        world.insert_resource(VehicleLibrary::default());
    }
    for v in vehicles {
        world
            .resource_mut::<VehicleLibrary>()
            .0
            .insert(v.name.clone(), (*v).clone());
    }
    world.insert_resource(TrafficSetup { def: def.clone() });
}

pub struct TrafficPlugin;

impl Plugin for TrafficPlugin {
    fn name(&self) -> &'static str {
        "traffic"
    }
    fn dependencies(&self) -> Vec<&'static str> {
        vec!["vehicle", "world"]
    }
    fn build(&self, app: &mut App) {
        app.world.register::<TrafficAgent>();
        app.add_system(Stage::Agents, "traffic", traffic)
            .reads::<Pose>()
            .reads::<Velocity>()
            .reads::<Vehicle>()
            .writes::<VehicleInput>()
            .writes::<TrafficAgent>()
            .reads_resource("Roads")
            .reads_resource("VehicleLibrary")
            .writes_resource("TrafficStats");
    }
}

/// A route being driven.
struct Plan {
    route: Vec<LaneId>,
    path: Arc<Polyline>,
    /// Arc length along `path` where each route element ends.
    ends: Vec<f32>,
    /// Progress along `path`, m.
    s: f32,
    integral: f32,
    /// The route cannot be extended: stop at its end, then reappear elsewhere.
    dead_end: bool,
    /// Seconds spent (nearly) stopped.
    still_s: f32,
}

/// Traffic's own state between ticks (not part of snapshots).
struct TrafficState {
    rng: SimRng,
    /// How many AI vehicles to keep on the roads.
    target: u32,
    /// Ticks until the next try at adding missing vehicles.
    top_up_in: u32,
    clear_of_player_m: f32,
    models: Vec<String>,
    plans: BTreeMap<Entity, Plan>,
    /// Junction connectors claimed: by whom, and how far either side of the
    /// path their body reaches (half width plus swept widening), m.
    claims: BTreeMap<LaneId, (Entity, f32)>,
    /// Closest approach of two connectors' paths, m (0 if they cross or
    /// merge; infinite if they leave the same lane), memoised.
    approach: BTreeMap<(LaneId, LaneId), f32>,
    respawns: u32,
}

/// Something on the road that others must not hit.
struct Body {
    entity: Entity,
    position: Vec2,
    velocity: Vec2,
    rear: f32,
    width: f32,
}

const IDM: Idm = Idm {
    accel: 1.5,
    decel: 2.0,
    headway_s: 1.4,
    min_gap_m: 2.5,
};
/// Look this far ahead for vehicles to follow, m.
const LOOK_AHEAD_M: f32 = 80.0;
/// Extend the route when this little is left, m.
const EXTEND_WITHIN_M: f32 = 60.0;
/// Reappear elsewhere after this long stopped, s.
const STUCK_S: f32 = 45.0;
/// Deceleration from rolling resistance, drag and engine braking alone, m/s².
const COAST_DECEL: f32 = 0.3;

fn traffic(world: &mut World) {
    let Some(roads) = world.get_resource::<Roads>().map(|r| r.0.clone()) else {
        return;
    };
    let dt = world.resource::<wp_core::SimTime>().dt();
    if let Some(setup) = world.get_resource::<TrafficSetup>().map(|s| s.def.clone()) {
        world.remove_resource::<TrafficSetup>();
        let seed = world
            .get_resource_mut::<SimRng>()
            .map_or(0, |mut r| r.next_u64());
        world.insert_resource(TrafficState {
            rng: SimRng::fork(seed, "traffic"),
            target: setup.count,
            top_up_in: 60,
            clear_of_player_m: setup.clear_of_player_m,
            models: setup.vehicles.clone(),
            plans: BTreeMap::new(),
            claims: BTreeMap::new(),
            approach: BTreeMap::new(),
            respawns: 0,
        });
        for _ in 0..setup.count {
            spawn_one(world, &roads, None);
        }
    }
    if !world.has_resource::<TrafficState>() {
        return;
    }
    // Vehicles that found no clear place yet: try again once a second.
    let missing = {
        let mut st = world.resource_mut::<TrafficState>();
        st.top_up_in = st.top_up_in.saturating_sub(1);
        let due = st.top_up_in == 0 && (st.plans.len() as u32) < st.target;
        if st.top_up_in == 0 {
            st.top_up_in = 60;
        }
        due
    };
    if missing {
        spawn_one(world, &roads, None);
    }

    let bodies = bodies(world);
    let agents: Vec<Entity> = world
        .resource::<TrafficState>()
        .plans
        .keys()
        .copied()
        .collect();
    let mut inputs = Vec::with_capacity(agents.len());
    let mut respawn = Vec::new();
    let (mut speeds, mut waiting) = (0.0f32, 0u32);
    for e in agents {
        let (Some(pose), Some(vel), Some(def)) = (
            world.get::<Pose>(e).map(|p| *p),
            world.get::<Velocity>(e).map(|v| *v),
            world.get::<Vehicle>(e).and_then(|v| {
                world
                    .get_resource::<VehicleLibrary>()
                    .and_then(|l| l.0.get(&v.model).cloned())
            }),
        ) else {
            continue;
        };
        let pace = world.get::<TrafficAgent>(e).map_or(1.0, |a| a.pace);
        let mut state = world.resource_mut::<TrafficState>();
        let (input, outcome) = drive(&mut state, &roads, &bodies, e, pose, vel, &def, pace, dt);
        drop(state);
        let speed = vel.linear.dot(pose.forward());
        speeds += speed.max(0.0) * 3.6;
        match outcome {
            Outcome::Driving => {}
            Outcome::Waiting => waiting += 1,
            Outcome::TripDone => {
                if let Some(mut a) = world.get_mut::<TrafficAgent>(e) {
                    a.trips += 1;
                }
            }
            Outcome::Respawn => respawn.push(e),
        }
        inputs.push((e, input));
    }
    for (e, input) in inputs {
        if let Some(mut i) = world.get_mut::<VehicleInput>(e) {
            *i = input;
        }
    }
    for e in respawn {
        spawn_one(world, &roads, Some(e));
    }
    let state = world.resource::<TrafficState>();
    let n = state.plans.len() as u32;
    let respawns = state.respawns;
    drop(state);
    let trips = world
        .query::<&TrafficAgent>()
        .entities()
        .into_iter()
        .filter_map(|e| world.get::<TrafficAgent>(e).map(|a| a.trips))
        .sum();
    world.insert_reflected_resource(
        "TrafficStats",
        TrafficStats {
            vehicles: n,
            trips,
            respawns,
            waiting,
            mean_kmh: if n > 0 { speeds / n as f32 } else { 0.0 },
        },
    );
}

/// Every vehicle and trailer, as obstacles.
fn bodies(world: &World) -> Vec<Body> {
    let mut out = Vec::new();
    let vehicles = world.get_resource::<VehicleLibrary>();
    world.query::<(&Pose, &Vehicle)>().for_each(|e, (pose, v)| {
        if let Some(def) = vehicles.as_ref().and_then(|l| l.0.get(&v.model)) {
            let fp = vehicle_footprint(def);
            let velocity = world.get::<Velocity>(e).map_or(Vec2::ZERO, |v| v.linear);
            out.push(Body {
                entity: e,
                position: pose.position,
                velocity,
                rear: fp.rear,
                width: fp.width,
            });
        }
    });
    let trailers = world.get_resource::<TrailerLibrary>();
    world.query::<(&Pose, &Trailer)>().for_each(|e, (pose, t)| {
        if let Some(def) = trailers.as_ref().and_then(|l| l.0.get(&t.model)) {
            let front = def.cg_behind_kingpin_m + def.body.kingpin_from_front_m;
            let velocity = world.get::<Velocity>(e).map_or(Vec2::ZERO, |v| v.linear);
            out.push(Body {
                entity: e,
                position: pose.position,
                velocity,
                rear: front - def.body.length_m,
                width: def.body.width_m,
            });
        }
    });
    out
}

enum Outcome {
    Driving,
    /// Stopped at a junction's stop line for its turn.
    Waiting,
    TripDone,
    Respawn,
}

/// One tick of one AI driver: its controls, and what happened.
#[allow(clippy::too_many_arguments)]
fn drive(
    state: &mut TrafficState,
    net: &RoadNetwork,
    bodies: &[Body],
    me: Entity,
    pose: Pose,
    vel: Velocity,
    def: &VehicleDef,
    pace: f32,
    dt: f32,
) -> (VehicleInput, Outcome) {
    let hold = VehicleInput {
        brake: 1.0,
        ..Default::default()
    };
    let speed = vel.linear.dot(pose.forward());
    let mut outcome = Outcome::Driving;

    // Progress along the route, extending it before it runs out.
    {
        let Some(plan) = state.plans.get_mut(&me) else {
            return (hold, Outcome::Driving);
        };
        let (s, _, _) =
            plan.path
                .project_between(pose.position, plan.s - TRACK_BACK_M, plan.s + TRACK_AHEAD_M);
        plan.s = s;
        plan.still_s = if speed.abs() < 0.5 {
            plan.still_s + dt
        } else {
            0.0
        };
    }
    let remaining = {
        let plan = &state.plans[&me];
        plan.path.length() - plan.s
    };
    if remaining < EXTEND_WITHIN_M && !state.plans[&me].dead_end {
        if extend(state, net, me) {
            outcome = Outcome::TripDone;
        } else if let Some(p) = state.plans.get_mut(&me) {
            p.dead_end = true;
        }
    }
    let plan = &state.plans[&me];
    let (path, s) = (plan.path.clone(), plan.s);
    let remaining = path.length() - s;
    if plan.still_s > STUCK_S || (plan.dead_end && remaining < 3.0) {
        release_claims(state, me);
        return (hold, Outcome::Respawn);
    }

    // The speed this driver wants: lane limits ahead (braking gently for
    // lower ones), bends, and the end of a dead-end route.
    let k = plan
        .ends
        .partition_point(|&e| e <= s)
        .min(plan.route.len() - 1);
    let limit = |i: usize| net.lanes[plan.route[i]].speed_limit_kmh / 3.6;
    let mut v0 = limit(k);
    for j in k + 1..plan.route.len() {
        let start = plan.ends[j - 1];
        if start - s > LOOK_AHEAD_M {
            break;
        }
        v0 = v0.min((limit(j).powi(2) + 2.0 * 1.5 * (start - s).max(0.0)).sqrt());
    }
    let to_stop = if plan.dead_end { remaining } else { 1e4 };
    let v0 = route_speed_kmh(&path, s, v0 * 3.6 * pace, to_stop) / 3.6;

    // The nearest thing ahead in this lane.
    let fp = vehicle_footprint(def);
    let (mut gap, mut lead_v) = (f32::INFINITY, 0.0);
    let lane_half = net.lanes[plan.route[k]].width_m / 2.0;
    // Changing lanes, or in a lane that closes ahead: yield to anything
    // alongside in the lane being entered.
    let changing = (k + 1 < plan.route.len()
        && net.lanes[plan.route[k]]
            .changes
            .contains(&plan.route[k + 1]))
        || (k > 0
            && net.lanes[plan.route[k - 1]]
                .changes
                .contains(&plan.route[k]))
        || lane_closes(net, plan.route[k]);
    for b in bodies {
        if b.entity == me || b.position.distance(pose.position) > LOOK_AHEAD_M + 20.0 {
            continue;
        }
        // A merging driver watches the whole lane being entered, not just
        // its own path, so it slots in behind whoever is there in good time.
        let (back, reach) = if changing {
            (fp.rear + b.rear - 2.0, 3.0 * lane_half)
        } else {
            (0.5, lane_half)
        };
        let (sb, lat, _) = path.project_between(b.position, s + back, s + LOOK_AHEAD_M);
        if sb <= s + back || lat.abs() > reach + b.width / 2.0 - 0.4 {
            continue;
        }
        if sb <= s + 0.5 {
            // Alongside: give way by dropping back.
            gap = gap.min(0.1);
            lead_v = 0.0;
            continue;
        }
        let g = sb - s - fp.front + b.rear;
        if g < gap {
            gap = g;
            let (_, dir) = path.sample(sb);
            lead_v = b.velocity.dot(dir);
        }
    }

    // Junctions: claim the next path across before reaching it; if it
    // cannot be claimed, the stop line acts as a stopped car.
    let ends = plan.ends.clone();
    let route = plan.route.clone();
    let mut stop_line: Option<f32> = None;
    for j in k..route.len() {
        if !matches!(net.lanes[route[j]].kind, LaneKind::Connector { .. }) {
            continue;
        }
        let entry = if j == 0 { 0.0 } else { ends[j - 1] };
        let ahead = entry - s;
        if ahead > speed.max(0.0).powi(2) / (2.0 * IDM.decel) + 15.0 {
            break; // not near yet
        }
        if state.claims.get(&route[j]).is_some_and(|c| c.0 == me) {
            continue; // already ours
        }
        if gap < ahead - fp.front + 2.0 {
            // Queued behind someone short of the junction: claim nothing yet
            // (holding a claim from the queue would block crossing traffic).
            break;
        }
        let reach = envelope(net, def, route[j]);
        if j == k && ahead < 0.0 {
            // Already on it (e.g. spawned there): take it.
            state.claims.insert(route[j], (me, reach));
            continue;
        }
        let exit = ends[j];
        let blocked_exit = bodies.iter().any(|b| {
            if b.entity == me || b.position.distance(pose.position) > LOOK_AHEAD_M + 40.0 {
                return false;
            }
            let (sb, lat, _) =
                path.project_between(b.position, exit, exit + fp.front - fp.rear + 6.0);
            sb > exit && lat.abs() < lane_half + 0.5 && b.velocity.length() < 2.0
        });
        let blockers = blocking_claims(state, net, route[j], me, reach);
        if !blocked_exit && blockers.is_empty() {
            state.claims.insert(route[j], (me, reach));
        } else {
            // Wait short of where any blocking vehicle's swept path would
            // reach this vehicle's approach (long vehicles swing wide).
            let mut line = entry;
            for (other, other_reach) in blockers {
                // Its path across, and the start of the lane it leaves into
                // (a long vehicle is still swinging for a body length).
                let mut pts = net.lanes[other].path.points.clone();
                if let Some(&next) = net.lanes[other].next.first() {
                    let exit = &net.lanes[next].path;
                    pts.extend(exit.slice(0.0, 15.0).points);
                }
                let mut x = (entry - 30.0).max(s);
                while x < line {
                    let (p, _) = path.sample(x);
                    if pts.iter().any(|q| q.distance(p) < reach + other_reach) {
                        line = x;
                        break;
                    }
                    x += 1.0;
                }
            }
            stop_line = Some(line);
            break;
        }
    }
    // Let go of connectors this vehicle has fully left.
    let behind = s + fp.rear;
    let mine: Vec<LaneId> = state
        .claims
        .iter()
        .filter(|(_, c)| c.0 == me)
        .map(|(l, _)| *l)
        .collect();
    for l in mine {
        let left = route
            .iter()
            .position(|&r| r == l)
            .is_none_or(|j| ends[j] < behind);
        if left {
            state.claims.remove(&l);
        }
    }
    if let Some(entry) = stop_line {
        let g = entry - s - fp.front - 1.0;
        if g < gap {
            gap = g.max(0.1);
            lead_v = 0.0;
        }
        if speed < 1.0 && g < 6.0 {
            outcome = Outcome::Waiting;
        }
    }

    // Pedals from the IDM acceleration; steering along the path. Slowing
    // beyond what rolling and drag give is brake pressure in proportion to
    // this vehicle's brakes (no switching back and forth between modes).
    let a = IDM.accel(speed, v0, gap, speed - lead_v);
    let plan = state.plans.get_mut(&me).expect("planned above");
    let (throttle, brake) = if speed < 0.3 && a <= 0.0 {
        plan.integral = 0.0;
        (0.0, 1.0)
    } else if a < -COAST_DECEL {
        plan.integral = 0.0;
        let capacity = def.brakes.max_torque_nm / def.wheel.radius_m / def.mass_kg;
        (
            0.0,
            ((-a - COAST_DECEL) / capacity.max(1.0)).clamp(0.0, 1.0),
        )
    } else {
        let target = (speed + a).max(0.0) * 3.6;
        let (throttle, _) = speed_control(&mut plan.integral, target, speed * 3.6, dt);
        (throttle, 0.0)
    };
    // Long vehicles swing wide: steering the front axle along the lane
    // would let the rear wheels cut inside by L²/2R, so aim to put the
    // middle of the wheelbase on it (shift L²/4R outwards). Wide vehicles
    // also keep towards the kerb, away from oncoming traffic.
    let front_axle = pose.position + pose.forward() * def.cg_to_front_axle_m;
    let (_, t0) = path.sample(s - 3.0);
    let (_, t1) = path.sample(s + 3.0);
    let (_, dir) = path.sample(s);
    let curvature = t0.angle_to(t1) / 6.0; // + bends left
    let left = Vec2::new(-dir.y, dir.x);
    let swing = curvature * def.wheelbase_m.powi(2) / 4.0;
    let kerb_side = match net.drive_side {
        wp_road::DriveSide::Right => -1.0,
        wp_road::DriveSide::Left => 1.0,
    };
    let lane_w = net.lanes[route[k]].width_m;
    let shy =
        kerb_side * ((lane_w - fp.width) / 2.0 * 0.5).max(0.0) * (fp.width > 2.2) as u8 as f32;
    // Stanley steers the axle onto the path; moving where it "sees" the
    // axle by -offset makes it hold the axle `offset` off the path.
    let seen = front_axle + left * (swing - shy);
    let steer = stanley_steering(&path, seen, pose.heading, speed.max(0.0), s);
    let max_steer = def.steering.max_angle_deg.to_radians();
    (
        VehicleInput {
            throttle,
            brake,
            steer: (steer / max_steer).clamp(-1.0, 1.0),
            reverse: false,
        },
        outcome,
    )
}

/// Whether a road lane narrows to nothing at its far end (a lane drop).
fn lane_closes(net: &RoadNetwork, lane: LaneId) -> bool {
    let LaneKind::Road {
        road,
        forward,
        index,
    } = net.lanes[lane].kind
    else {
        return false;
    };
    // Forward lanes run towards the road's end, backward lanes towards its start.
    let taper = &net.roads[road].tapers[if forward { 1 } else { 0 }];
    taper.as_ref().is_some_and(|t| {
        let lanes = if forward { &t.forward } else { &t.backward };
        lanes.get(index as usize).is_some_and(|l| l.1)
    })
}

/// How far either side of its path a vehicle's body reaches on connector
/// `lane`: half its width, plus the widening of its swept path on the
/// tightest part of the turn (rear wheels cutting in by about L²/2R, the
/// front overhang swinging out), plus a margin.
fn envelope(net: &RoadNetwork, def: &VehicleDef, lane: LaneId) -> f32 {
    let fp = vehicle_footprint(def);
    let r = min_radius(&net.lanes[lane].path).max(3.0);
    let cut_in = def.wheelbase_m.powi(2) / (2.0 * r);
    let swing = (fp.front - def.cg_to_front_axle_m).max(0.0).powi(2) / (2.0 * r);
    fp.width / 2.0 + cut_in.max(swing) + 0.3
}

/// Tightest radius along a path, m.
fn min_radius(path: &Polyline) -> f32 {
    let p = &path.points;
    (1..p.len().saturating_sub(1))
        .map(|i| {
            let (a, b) = (p[i] - p[i - 1], p[i + 1] - p[i]);
            let turn = a.angle_to(b).abs();
            if turn < 1e-6 {
                f32::INFINITY
            } else {
                (a.length() + b.length()) / 2.0 / turn
            }
        })
        .fold(f32::INFINITY, f32::min)
}

/// Claims by other vehicles whose bodies would come too close to ours on
/// connector `lane`: (their connector, how far their body reaches). Empty if
/// `lane` can be claimed.
fn blocking_claims(
    state: &mut TrafficState,
    net: &RoadNetwork,
    lane: LaneId,
    me: Entity,
    reach: f32,
) -> Vec<(LaneId, f32)> {
    let others: Vec<(LaneId, f32)> = state
        .claims
        .iter()
        .filter(|(_, c)| c.0 != me)
        .map(|(l, c)| (*l, c.1))
        .collect();
    others
        .into_iter()
        .filter(|&(other, other_reach)| {
            let key = if lane < other {
                (lane, other)
            } else {
                (other, lane)
            };
            let gap = *state
                .approach
                .entry(key)
                .or_insert_with(|| closest_approach(net, lane, other));
            gap < reach + other_reach
        })
        .collect()
}

/// Closest approach of two connectors' paths at one junction, m: 0 if they
/// cross or lead into the same lane, infinite if they leave the same lane
/// (one follows the other) or belong to different junctions.
fn closest_approach(net: &RoadNetwork, a: LaneId, b: LaneId) -> f32 {
    let (la, lb) = (&net.lanes[a], &net.lanes[b]);
    let same_node = matches!((la.kind, lb.kind), (LaneKind::Connector { node: x, .. }, LaneKind::Connector { node: y, .. }) if x == y);
    if !same_node {
        return f32::INFINITY;
    }
    if la.next == lb.next {
        return 0.0;
    }
    let (pa, pb) = (&la.path.points, &lb.path.points);
    if pa
        .first()
        .zip(pb.first())
        .is_some_and(|(x, y)| x.distance(*y) < 0.5)
    {
        return f32::INFINITY;
    }
    for i in 1..pa.len() {
        for j in 1..pb.len() {
            if segments_cross(pa[i - 1], pa[i], pb[j - 1], pb[j]) {
                return 0.0;
            }
        }
    }
    pa.iter()
        .flat_map(|p| pb.iter().map(move |q| p.distance(*q)))
        .fold(f32::INFINITY, f32::min)
}

fn segments_cross(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let side = |p: Vec2, q: Vec2, r: Vec2| (q - p).perp_dot(r - p);
    let (d1, d2) = (side(c, d, a), side(c, d, b));
    let (d3, d4) = (side(a, b, c), side(a, b, d));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

fn release_claims(state: &mut TrafficState, me: Entity) {
    state.claims.retain(|_, c| c.0 != me);
}

/// A random road reachable from `from`, and the route there.
fn random_route(rng: &mut SimRng, net: &RoadNetwork, from: LaneId) -> Option<Vec<LaneId>> {
    for _ in 0..12 {
        let road = (rng.next_u64() % net.roads.len() as u64) as usize;
        let id = &net.roads[road].def.id;
        if matches!(net.lanes[from].kind, LaneKind::Road { road: r, .. } if r == road) {
            continue;
        }
        if let Some(route) = net.route(from, id) {
            return Some(route);
        }
    }
    None
}

/// Where each route element ends along the path built by `route_path`.
fn element_ends(net: &RoadNetwork, route: &[LaneId], start_s: f32) -> Vec<f32> {
    let mut ends = Vec::with_capacity(route.len());
    let mut acc = 0.0;
    let mut i = 0;
    while i < route.len() {
        // A run of lane changes counts once, as the first lane's length.
        let mut j = i;
        while j + 1 < route.len() && net.lanes[route[j]].changes.contains(&route[j + 1]) {
            j += 1;
        }
        let len = net.lanes[route[i]].path.length() - if i == 0 { start_s } else { 0.0 };
        acc += len.max(0.0);
        for _ in i..=j {
            ends.push(acc);
        }
        i = j + 1;
    }
    ends
}

fn new_plan(net: &RoadNetwork, route: Vec<LaneId>, start_s: f32) -> Plan {
    let path = net.route_path(&route, start_s);
    Plan {
        ends: element_ends(net, &route, start_s),
        route,
        path: Arc::new(path),
        s: 0.0,
        integral: 0.0,
        dead_end: false,
        still_s: 0.0,
    }
}

/// Re-plan from the current route element to a new destination. False if
/// there is nowhere to go from the route's end (a dead end).
fn extend(state: &mut TrafficState, net: &RoadNetwork, me: Entity) -> bool {
    let plan = &state.plans[&me];
    let k = plan
        .ends
        .partition_point(|&e| e <= plan.s)
        .min(plan.route.len() - 1);
    let last = *plan.route.last().expect("routes are never empty");
    let Some(more) = random_route(&mut state.rng, net, last) else {
        return false;
    };
    let plan = &state.plans[&me];
    let element_start = if k == 0 { 0.0 } else { plan.ends[k - 1] };
    // Within the current element: how far along it the vehicle is.
    let first_offset = if k == 0 {
        net.lanes[plan.route[0]].path.length() - plan.ends[0]
    } else {
        0.0
    };
    let start_s = plan.s - element_start + first_offset;
    let mut route: Vec<LaneId> = plan.route[k..].to_vec();
    route.extend_from_slice(&more[1..]);
    let integral = plan.integral;
    let mut fresh = new_plan(net, route, start_s.max(0.0));
    fresh.integral = integral;
    state.plans.insert(me, fresh);
    true
}

/// Place an AI vehicle somewhere clear on the roads: a new one, or `reuse`
/// moved from where it was.
fn spawn_one(world: &mut World, net: &RoadNetwork, reuse: Option<Entity>) {
    let player = world
        .query::<&Player>()
        .entities()
        .first()
        .and_then(|e| world.get::<Pose>(*e).map(|p| p.position));
    let others: Vec<Vec2> = bodies(world)
        .iter()
        .filter(|b| Some(b.entity) != reuse)
        .map(|b| b.position)
        .collect();
    let candidates: Vec<LaneId> = (0..net.lanes.len())
        .filter(|&l| {
            matches!(net.lanes[l].kind, LaneKind::Road { .. }) && net.lanes[l].path.length() > 60.0
        })
        .collect();
    if candidates.is_empty() {
        return;
    }
    let mut state = world.resource_mut::<TrafficState>();
    let clear = state.clear_of_player_m;
    let mut found = None;
    for _ in 0..60 {
        let lane = candidates[(state.rng.next_u64() % candidates.len() as u64) as usize];
        let len = net.lanes[lane].path.length();
        let s = state.rng.range_f32(25.0, len - 25.0);
        let (p, dir) = net.lanes[lane].path.sample(s);
        if player.is_some_and(|q| q.distance(p) < clear)
            || others.iter().any(|q| q.distance(p) < 25.0)
        {
            continue;
        }
        let Some(route) = random_route(&mut state.rng, net, lane) else {
            continue;
        };
        found = Some((lane, s, p, dir, route));
        break;
    }
    let Some((lane, s, p, dir, route)) = found else {
        // Nowhere clear right now: it stays put and tries again next tick.
        return;
    };
    if state.models.is_empty() {
        return;
    }
    let pick = (state.rng.next_u64() % state.models.len() as u64) as usize;
    let model = state.models[pick].clone();
    let pace = state.rng.range_f32(0.85, 1.05);
    let limit = net.lanes[lane].speed_limit_kmh;
    if reuse.is_some() {
        state.respawns += 1;
    }
    let plan = new_plan(net, route, s);
    drop(state);
    let def = world.resource::<VehicleLibrary>().0.get(&model).cloned();
    let Some(def) = def else { return };
    let heading = dir.y.atan2(dir.x);
    let e = match reuse {
        Some(e) => {
            world.insert(
                e,
                Pose {
                    position: p,
                    heading,
                },
            );
            world.insert(e, Velocity::default());
            // It may come back as a different model: its body too.
            world.insert(e, RigidBody::new(def.mass_kg, def.yaw_inertia_kgm2));
            world.insert(e, Vehicle::new(&def));
            world.insert(e, VehicleInput::default());
            e
        }
        None => {
            let e = wp_vehicle::spawn_vehicle(world, &def, p, heading);
            world.insert(e, wp_world::Name(format!("traffic {model}")));
            world.insert(e, TrafficAgent { trips: 0, pace });
            e
        }
    };
    // Already moving with the traffic around it.
    wp_vehicle::set_rolling_speed(world, e, (limit / 3.6) * 0.6);
    let mut state = world.resource_mut::<TrafficState>();
    release_claims(&mut state, e);
    state.plans.insert(e, plan);
}

/// A one-line description of an AI vehicle's plan, for debugging: the
/// current route element, progress, path length and the next few lanes.
pub fn describe(world: &World, e: Entity) -> Option<String> {
    let state = world.get_resource::<TrafficState>()?;
    let net = world.get_resource::<Roads>()?.0.clone();
    let plan = state.plans.get(&e)?;
    let k = plan
        .ends
        .partition_point(|&x| x <= plan.s)
        .min(plan.route.len() - 1);
    let names: Vec<String> = plan.route[k..(k + 4).min(plan.route.len())]
        .iter()
        .map(|&l| net.lanes[l].name.clone())
        .collect();
    let (p, _) = plan.path.sample(plan.s);
    Some(format!(
        "s {:.1}/{:.1} elem {k}/{} [{}] path@s ({:.1},{:.1}){}",
        plan.s,
        plan.path.length(),
        plan.route.len(),
        names.join(" "),
        p.x,
        p.y,
        if plan.dead_end { " DEAD-END" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wp_road::{DriveSide, NodeDef, RoadDef, RoadNetworkDef, RoadShape};

    /// A crossroads with arms 100 m long, one lane each way.
    fn crossroads() -> RoadNetwork {
        let node = |id: &str, x: f32, y: f32| NodeDef {
            id: id.into(),
            position: (x, y),
        };
        let road = |id: &str, from: &str, to: &str| RoadDef {
            id: id.into(),
            name: None,
            from: from.into(),
            to: to.into(),
            shape: RoadShape::Straight,
            lanes_forward: 1,
            lanes_backward: 1,
            lane_width_m: 3.5,
            speed_limit_kmh: 50.0,
            sidewalk_m: 0.0,
        };
        RoadNetwork::build(&RoadNetworkDef {
            name: "x".into(),
            drive_side: DriveSide::Right,
            nodes: vec![
                node("c", 0.0, 0.0),
                node("n", 0.0, 100.0),
                node("e", 100.0, 0.0),
                node("s", 0.0, -100.0),
                node("w", -100.0, 0.0),
            ],
            roads: vec![
                road("north", "c", "n"),
                road("east", "c", "e"),
                road("south", "s", "c"),
                road("west", "w", "c"),
            ],
        })
    }

    /// The connector from lane `from` that leads into road `to`.
    fn connector(net: &RoadNetwork, from: &str, to: &str) -> LaneId {
        let l = net.lane(from).unwrap();
        *net.lanes[l]
            .next
            .iter()
            .find(|&&c| net.lanes[net.lanes[c].next[0]].name.starts_with(to))
            .unwrap()
    }

    #[test]
    fn crossing_paths_conflict_and_parallel_ones_do_not() {
        let net = crossroads();
        // Westbound straight on (from the east road) crosses northbound straight on.
        let west_on = connector(&net, "east:b0", "west");
        let north_on = connector(&net, "south:f0", "north");
        assert_eq!(closest_approach(&net, west_on, north_on), 0.0);
        // Two straight-on paths in opposite directions pass a lane apart.
        let east_on = connector(&net, "west:f0", "east");
        let gap = closest_approach(&net, west_on, east_on);
        assert!(gap > 3.0 && gap < 4.0, "{gap}");
        // Leaving the same lane: one follows the other, no conflict.
        let east_right = connector(&net, "west:f0", "south");
        assert_eq!(closest_approach(&net, east_on, east_right), f32::INFINITY);
    }

    #[test]
    fn long_vehicles_reach_further_on_turns() {
        let net = crossroads();
        let load = |src: &str| -> VehicleDef { wp_data::parse_ron("v.ron", src).unwrap() };
        let car = load(include_str!(
            "../../../examples/sandbox/vehicles/hatchback.vehicle.ron"
        ));
        let bus = load(include_str!(
            "../../../examples/sandbox/vehicles/city_bus.vehicle.ron"
        ));
        let turn = connector(&net, "west:f0", "north");
        let (c, b) = (envelope(&net, &car, turn), envelope(&net, &bus, turn));
        assert!(c < 1.8 && b > c + 1.5, "car {c} m, bus {b} m");
        // Two cars may pass in opposite directions straight on; two buses turning may not.
        let west_on = connector(&net, "east:b0", "west");
        let east_on = connector(&net, "west:f0", "east");
        assert!(closest_approach(&net, west_on, east_on) > 2.0 * envelope(&net, &car, east_on));
    }

    #[test]
    fn element_ends_count_a_lane_change_run_once() {
        let net = crossroads();
        let route = net.route(net.lane("west:f0").unwrap(), "north").unwrap();
        let ends = element_ends(&net, &route, 10.0);
        let path = net.route_path(&route, 10.0);
        assert!(
            (ends.last().unwrap() - path.length()).abs() < 0.5,
            "{ends:?} vs {}",
            path.length()
        );
        assert!(ends.windows(2).all(|w| w[1] >= w[0]));
    }
}
