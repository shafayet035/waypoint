//! Pedestrians: people who walk the sidewalk graph to random places. They
//! cross roads only at zebra crossings: at signals while the traffic on that
//! road has a red light (and will for the whole crossing); elsewhere when no
//! vehicle is on or approaching the crossing within about five seconds.
//! Vehicles treat people on the road as obstacles (see the traffic module).
//! A vehicle touching a person knocks them down and sends `HitPedestrian`.

use glam::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wp_core::SimRng;
use wp_ecs::{Component, Entity, Events, World};
use wp_physics::{Collider, Pose, Velocity};
use wp_road::{Light, RoadNetwork, WalkKind};
use wp_vehicle::{Vehicle, VehicleEvent};
use wp_world::Roads;

/// A person walking.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pedestrian {
    /// Walking speed, m/s.
    pub speed: f32,
    /// Knocked down by a vehicle (lies still for a while, then gets up elsewhere).
    pub down: bool,
}

impl Component for Pedestrian {
    const NAME: &'static str = "Pedestrian";
}

/// How the pedestrians are doing (`world.resources` shows it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PedestrianStats {
    pub walking: u32,
    /// Roads crossed at zebra crossings.
    pub crossings: u32,
    /// People hit by vehicles.
    pub hits: u32,
    /// People waiting at a crossing right now.
    pub waiting: u32,
}

/// What to spawn, from the scene.
#[derive(Clone, Debug)]
pub struct PedestrianSetup {
    pub count: u32,
}

struct Walk {
    /// Edges of the sidewalk graph and the way each is walked.
    route: Vec<(usize, bool)>,
    /// Current edge and distance along it (in the walking direction).
    idx: usize,
    s: f32,
    /// Seconds lying down after being hit.
    down_s: f32,
    /// How far they stand back from the kerb while waiting, m.
    back_m: f32,
    /// How far they have stepped aside (left of the way they walk) on a
    /// sidewalk, m.
    aside_m: f32,
}

struct PedestrianState {
    rng: SimRng,
    walks: BTreeMap<Entity, Walk>,
    crossings: u32,
    hits: u32,
}

/// Seconds a vehicle must be from a crossing before someone steps out.
const SAFE_GAP_S: f32 = 5.0;
const DOWN_FOR_S: f32 = 8.0;
/// Waiting at a kerb, people step back this far from a moving vehicle whose
/// body comes within `STAND_BACK_WITHIN_M` of where they stand (a long
/// vehicle's front swinging round a corner); walking along a sidewalk, they
/// step aside as far.
const STAND_BACK_M: f32 = 0.9;
const STAND_BACK_WITHIN_M: f32 = 1.2;

/// Move every pedestrian one tick (spawning them on the first).
pub fn pedestrians(world: &mut World) {
    let Some(net) = world.get_resource::<Roads>().map(|r| r.0.clone()) else {
        return;
    };
    if net.sidewalks.edges.is_empty() {
        return;
    }
    let dt = world.resource::<wp_core::SimTime>().dt();
    let now = world.resource::<wp_core::SimTime>().seconds();
    if let Some(setup) = world.remove_resource::<PedestrianSetup>() {
        let seed = world
            .get_resource_mut::<SimRng>()
            .map_or(0, |mut r| r.next_u64());
        world.insert_resource(PedestrianState {
            rng: SimRng::fork(seed, "pedestrians"),
            walks: BTreeMap::new(),
            crossings: 0,
            hits: 0,
        });
        for _ in 0..setup.count {
            place(world, &net, None);
        }
    }
    if !world.has_resource::<PedestrianState>() {
        return;
    }
    // Vehicles: position, velocity, how far their body reaches.
    let mut vehicles: Vec<(Vec2, Vec2, f32)> = Vec::new();
    // Bodies of the moving ones, for stepping back.
    let mut moving: Vec<[Vec2; 4]> = Vec::new();
    world.query::<(&Pose, &Vehicle)>().for_each(|e, (pose, _)| {
        let v = world.get::<Velocity>(e).map_or(Vec2::ZERO, |v| v.linear);
        let c = world.get::<Collider>(e).map(|c| *c);
        vehicles.push((pose.position, v, c.map_or(3.0, |c| c.radius())));
        if let Some(c) = c
            && v.length() > 0.5
        {
            moving.push(c.corners(pose));
        }
    });
    let people: Vec<Entity> = world
        .resource::<PedestrianState>()
        .walks
        .keys()
        .copied()
        .collect();
    let mut moves = Vec::new();
    let mut respawn = Vec::new();
    let mut waiting = 0;
    for e in people {
        let Some(me) = world.get::<Pedestrian>(e).map(|p| p.clone()) else {
            continue;
        };
        let mut st = world.resource_mut::<PedestrianState>();
        let PedestrianState {
            rng,
            walks,
            crossings,
            ..
        } = &mut *st;
        let walk = walks.get_mut(&e).expect("listed above");
        if me.down {
            walk.down_s += dt;
            if walk.down_s > DOWN_FOR_S {
                respawn.push(e);
            }
            continue;
        }
        let (edge_id, fwd) = walk.route[walk.idx];
        let edge = &net.sidewalks.edges[edge_id];
        // At the kerb of a crossing: go only when it is safe.
        if let WalkKind::Crossing { road, node } = edge.kind
            && walk.s == 0.0
        {
            let cross_s = edge.path.length() / me.speed.max(0.5);
            let (a, b) = (
                edge.path.points[0],
                *edge.path.points.last().expect("2 points"),
            );
            let clear = || {
                vehicles.iter().all(|&(p, v, r)| {
                    // Distance from the vehicle's body to the crossing,
                    // and how soon it could be there at its speed.
                    let along = (p - a).dot(b - a) / (b - a).length_squared().max(1e-6);
                    let d = p.distance(a + (b - a) * along.clamp(0.0, 1.0)) - r;
                    let speed = v.length();
                    d > 1.5 && (speed < 0.5 || d / speed > SAFE_GAP_S.max(cross_s))
                })
            };
            let safe = match net
                .junction_at(node)
                .and_then(|j| j.light(road, now).map(|l| (j, l)))
            {
                // Red for the whole crossing, and nothing still coming
                // through (a car that entered on amber, a turning car).
                Some((j, Light::Red)) => {
                    j.light(road, now + cross_s as f64 + 1.0) == Some(Light::Red) && clear()
                }
                Some(_) => false,
                None => clear(),
            };
            // Step back from a body swinging close; go only from the kerb.
            let (p, dir) = point_on(&net, edge_id, fwd, 0.0);
            let close = moving.iter().any(|q| near_box(p, q, STAND_BACK_WITHIN_M));
            let back = if close { STAND_BACK_M } else { 0.0 };
            walk.back_m += (back - walk.back_m).clamp(-me.speed * dt, me.speed * dt);
            if !safe || walk.back_m > 0.0 {
                waiting += 1;
                moves.push((e, p - dir * walk.back_m, dir, 0.0));
                continue;
            }
            *crossings += 1;
        }
        // Walk on, edge to edge; at the end, somewhere new.
        walk.s += me.speed * dt;
        while walk.s >= net.sidewalks.edges[walk.route[walk.idx].0].path.length() {
            walk.s -= net.sidewalks.edges[walk.route[walk.idx].0].path.length();
            walk.idx += 1;
            if walk.idx == walk.route.len() {
                let (last, last_fwd) = walk.route[walk.route.len() - 1];
                let at = if last_fwd {
                    net.sidewalks.edges[last].to
                } else {
                    net.sidewalks.edges[last].from
                };
                walk.route = random_walk(rng, &net, at).unwrap_or_else(|| vec![(last, !last_fwd)]);
                walk.idx = 0;
                walk.s = 0.0;
                break;
            }
            // Stop at the kerb of a crossing to look first.
            if matches!(
                net.sidewalks.edges[walk.route[walk.idx].0].kind,
                WalkKind::Crossing { .. }
            ) {
                walk.s = 0.0;
                break;
            }
        }
        let (id, f) = walk.route[walk.idx];
        let (p, dir) = point_on(&net, id, f, walk.s);
        // On a sidewalk, step aside from a body swinging over the kerb.
        let left = Vec2::new(-dir.y, dir.x);
        let mut aside = 0.0;
        if matches!(net.sidewalks.edges[id].kind, WalkKind::Sidewalk)
            && let Some(q) = moving.iter().find(|q| near_box(p, q, STAND_BACK_WITHIN_M))
        {
            let centre = (q[0] + q[1] + q[2] + q[3]) / 4.0;
            aside = STAND_BACK_M
                * if left.dot(p - centre) >= 0.0 {
                    1.0
                } else {
                    -1.0
                };
        }
        walk.aside_m += (aside - walk.aside_m).clamp(-me.speed * dt, me.speed * dt);
        moves.push((e, p + left * walk.aside_m, dir, me.speed));
    }
    for (e, p, dir, speed) in moves {
        if let Some(mut pose) = world.get_mut::<Pose>(e) {
            pose.position = p;
            pose.heading = dir.y.atan2(dir.x);
        }
        if let Some(mut v) = world.get_mut::<Velocity>(e) {
            v.linear = dir * speed;
        }
    }
    for e in respawn {
        place(world, &net, Some(e));
    }
    let st = world.resource::<PedestrianState>();
    let stats = PedestrianStats {
        walking: st.walks.len() as u32,
        crossings: st.crossings,
        hits: st.hits,
        waiting,
    };
    drop(st);
    world.insert_reflected_resource("PedestrianStats", stats);
}

/// A point `s` metres along edge `id` walked forwards or backwards, and the
/// walking direction there.
fn point_on(net: &RoadNetwork, id: usize, fwd: bool, s: f32) -> (Vec2, Vec2) {
    let path = &net.sidewalks.edges[id].path;
    if fwd {
        path.sample(s)
    } else {
        let (p, d) = path.sample(path.length() - s);
        (p, -d)
    }
}

fn random_walk(rng: &mut SimRng, net: &RoadNetwork, from: usize) -> Option<Vec<(usize, bool)>> {
    let n = net.sidewalks.nodes.len();
    for _ in 0..8 {
        let to = (rng.next_u64() % n as u64) as usize;
        if to == from {
            continue;
        }
        if let Some(r) = net.sidewalks.route(from, to)
            && !r.is_empty()
        {
            return Some(r);
        }
    }
    None
}

/// Put a person at a random place on a sidewalk, walking somewhere: a new
/// one, or `reuse` (back on their feet).
fn place(world: &mut World, net: &RoadNetwork, reuse: Option<Entity>) {
    let mut st = world.resource_mut::<PedestrianState>();
    let edges: Vec<usize> = (0..net.sidewalks.edges.len())
        .filter(|&i| matches!(net.sidewalks.edges[i].kind, WalkKind::Sidewalk))
        .collect();
    if edges.is_empty() {
        return;
    }
    let edge = edges[(st.rng.next_u64() % edges.len() as u64) as usize];
    let start = net.sidewalks.edges[edge].from;
    let Some(route) = random_walk(&mut st.rng, net, start) else {
        return;
    };
    let speed = st.rng.range_f32(1.1, 1.6);
    let walk = Walk {
        route,
        idx: 0,
        s: 0.0,
        down_s: 0.0,
        back_m: 0.0,
        aside_m: 0.0,
    };
    drop(st);
    let (p, dir) = point_on(net, walk.route[0].0, walk.route[0].1, 0.0);
    let e = reuse.unwrap_or_else(|| world.spawn());
    world.insert(
        e,
        Pose {
            position: p,
            heading: dir.y.atan2(dir.x),
        },
    );
    world.insert(e, Velocity::default());
    world.insert(e, Pedestrian { speed, down: false });
    world
        .resource_mut::<PedestrianState>()
        .walks
        .insert(e, walk);
}

/// Whether person `e` is walking across a road on a crossing.
pub(crate) fn on_crossing(world: &World, e: Entity) -> bool {
    let (Some(st), Some(net)) = (
        world.get_resource::<PedestrianState>(),
        world.get_resource::<Roads>(),
    ) else {
        return false;
    };
    st.walks.get(&e).is_some_and(|w| {
        w.s > 0.0
            && matches!(
                net.0.sidewalks.edges[w.route[w.idx].0].kind,
                WalkKind::Crossing { .. }
            )
    })
}

/// Knock down people that vehicles touch, and tell the vehicle.
pub fn detect_pedestrian_hits(world: &mut World) {
    if !world.has_resource::<PedestrianState>() {
        return;
    }
    let mut boxes: Vec<(Entity, [Vec2; 4], Vec2, f32)> = Vec::new();
    world
        .query::<(&Pose, &Collider)>()
        .for_each(|e, (pose, c)| {
            // Only a moving vehicle hits someone; walking into a parked one
            // is not a road accident.
            let moving = world
                .get::<Velocity>(e)
                .is_some_and(|v| v.linear.length() > 0.5);
            if world.get::<Vehicle>(e).is_some() && moving {
                boxes.push((e, c.corners(pose), pose.position, c.radius()));
            }
        });
    let mut hits = Vec::new();
    world
        .query::<(&Pose, &Pedestrian)>()
        .for_each(|e, (pose, p)| {
            if p.down {
                return;
            }
            for (v, corners, centre, radius) in &boxes {
                if centre.distance(pose.position) < radius + 0.4
                    && near_box(pose.position, corners, 0.25)
                {
                    hits.push((*v, e));
                    break;
                }
            }
        });
    if hits.is_empty() {
        return;
    }
    for &(_, p) in &hits {
        if let Some(mut ped) = world.get_mut::<Pedestrian>(p) {
            ped.down = true;
        }
        if let Some(mut v) = world.get_mut::<Velocity>(p) {
            *v = Velocity::default();
        }
    }
    world.resource_mut::<PedestrianState>().hits += hits.len() as u32;
    if let Some(mut events) = world.get_resource_mut::<Events<VehicleEvent>>() {
        for (v, p) in hits {
            events.send(VehicleEvent::HitPedestrian {
                entity: v.to_string(),
                pedestrian: p.to_string(),
            });
        }
    }
}

/// Whether `p` is inside the box `q`, or within `margin` of it.
fn near_box(p: Vec2, q: &[Vec2; 4], margin: f32) -> bool {
    let orient = (q[1] - q[0]).perp_dot(q[2] - q[0]).signum();
    (0..4).all(|i| {
        let edge = q[(i + 1) % 4] - q[i];
        edge.perp_dot(p - q[i]) * orient / edge.length().max(1e-6) >= -margin
    })
}
