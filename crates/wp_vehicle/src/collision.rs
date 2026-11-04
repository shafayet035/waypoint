//! Collision events between vehicles and trailers. The physics contact
//! solver (`wp_physics::contact`) makes bodies bump and push each other; this
//! reports, once per tick, which pairs have just started touching, as a
//! `Collision` event to each of them. A body that runs into a building
//! (`wp_physics::Walls`) gets a `Collision` with `other: "building"`.

use crate::{VehicleDef, VehicleEvent};
use glam::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use wp_ecs::{Component, Entity, Events, World};
use wp_physics::{Collider, PinJoint, Pose, RigidBody, Velocity};

/// Crash damage, added to a body the first time it is hit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Damage {
    /// Dents, most recent last (at most [`MAX_DENTS`]).
    pub dents: Vec<Dent>,
    /// 100 = as new, 0 = written off.
    pub health_pct: f32,
}

impl Component for Damage {
    const NAME: &'static str = "Damage";
}

/// A dent: where the body was pushed in, in the body's frame (x forward
/// from its pose, y left), and how far, m.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dent {
    pub x: f32,
    pub y: f32,
    pub depth_m: f32,
}

/// Dents kept per body; older ones merge into the record of health only.
pub const MAX_DENTS: usize = 12;

/// Dent depth for an impact at `kmh` (closing speed along the contact
/// normal) taking share `share` (0–1) of the crush: nothing below 5 km/h,
/// about 18 cm between equal cars at 30 km/h, capped at 70 cm.
pub fn dent_depth_m(kmh: f32, share: f32) -> f32 {
    ((kmh - 5.0).max(0.0) * 0.007 * share * 2.0).min(0.7)
}

/// A body's extent along its heading from its pose (front positive), and width, m.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Footprint {
    pub front: f32,
    pub rear: f32,
    pub width: f32,
}

impl Footprint {
    /// The four corners at `pose`.
    pub fn corners(&self, pose: &Pose) -> [Vec2; 4] {
        let f = pose.forward();
        let l = Vec2::new(-f.y, f.x) * (self.width / 2.0);
        let (a, b) = (
            pose.position + f * self.front,
            pose.position + f * self.rear,
        );
        [a + l, a - l, b - l, b + l]
    }
}

/// A vehicle's body footprint (also what is drawn): from the body
/// dimensions, or estimated from the wheels when it has no body.
pub fn vehicle_footprint(def: &VehicleDef) -> Footprint {
    if let Some(b) = &def.body {
        let front = def.cg_to_front_axle_m + b.front_overhang_m;
        return Footprint {
            front,
            rear: front - b.length_m,
            width: b.width_m,
        };
    }
    let r = def.wheel.radius_m;
    Footprint {
        front: def.cg_to_front_axle_m + r * 1.6 + 0.2,
        rear: -(def.rear_axle_from_cg() + r * 1.5 + 0.1),
        width: def.track_width_m + 0.24,
    }
}

/// Pairs of bodies in contact at the last check (lower entity first).
#[derive(Clone, Debug, Default)]
pub struct Contacts(pub BTreeSet<(Entity, Entity)>);

/// Bodies touching a building at the last check.
#[derive(Clone, Debug, Default)]
pub struct WallContacts(pub BTreeSet<Entity>);

/// What a body hit, as named in `Collision` events.
pub const BUILDING: &str = "building";

/// Separating-axis test for two convex quadrilaterals.
pub fn overlap(a: &[Vec2; 4], b: &[Vec2; 4]) -> bool {
    let axes = |q: &[Vec2; 4]| [q[1] - q[0], q[2] - q[1]];
    for axis in axes(a).into_iter().chain(axes(b)) {
        let n = Vec2::new(-axis.y, axis.x);
        let span = |q: &[Vec2; 4]| {
            q.iter()
                .map(|p| p.dot(n))
                .fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)))
        };
        let ((a0, a1), (b0, b1)) = (span(a), span(b));
        if a1 < b0 || b1 < a0 {
            return false;
        }
    }
    true
}

/// Find touching bodies (any with a physics [`Collider`]) and send
/// `Collision` events for new contacts. The contact solver pushes bodies
/// apart, so a contact is "touching within a few centimetres".
pub fn detect_collisions(world: &mut World) {
    let mut bodies: Vec<(Entity, [Vec2; 4], Vec2, f32)> = Vec::new();
    world
        .query::<(&Pose, &Collider)>()
        .for_each(|e, (pose, c)| {
            let grown = Collider {
                front: c.front + TOUCH_M,
                rear: c.rear - TOUCH_M,
                half_width: c.half_width + TOUCH_M,
            };
            bodies.push((e, grown.corners(pose), pose.position, grown.radius()));
        });
    // A tractor and its own trailer overlap at the fifth wheel by design.
    let coupled: BTreeSet<(Entity, Entity)> = world
        .query::<&PinJoint>()
        .entities()
        .into_iter()
        .filter_map(|e| world.get::<PinJoint>(e).map(|j| ordered(e, j.other)))
        .collect();
    let mut now = BTreeSet::new();
    for i in 0..bodies.len() {
        for j in i + 1..bodies.len() {
            let (a, b) = (&bodies[i], &bodies[j]);
            if a.2.distance(b.2) > a.3 + b.3 {
                continue; // too far apart to touch
            }
            let pair = ordered(a.0, b.0);
            if !coupled.contains(&pair) && overlap(&a.1, &b.1) {
                now.insert(pair);
            }
        }
    }
    let before = world
        .get_resource::<Contacts>()
        .map(|c| c.0.clone())
        .unwrap_or_default();
    let new: Vec<(Entity, Entity)> = now.difference(&before).copied().collect();
    world.insert_resource(Contacts(now));
    // Bodies against buildings.
    let mut walls_now: BTreeMap<Entity, [Vec2; 4]> = BTreeMap::new();
    if let Some(walls) = world.get_resource::<wp_physics::Walls>() {
        for (e, grown, pos, r) in &bodies {
            if let Some(w) = walls
                .near(*pos, *r)
                .into_iter()
                .find(|w| overlap(grown, &walls.boxes[*w as usize]))
            {
                walls_now.insert(*e, walls.boxes[w as usize]);
            }
        }
    }
    let walls_before = world
        .get_resource::<WallContacts>()
        .map(|c| c.0.clone())
        .unwrap_or_default();
    let new_walls: Vec<(Entity, [Vec2; 4])> = walls_now
        .iter()
        .filter(|(e, _)| !walls_before.contains(e))
        .map(|(e, q)| (*e, *q))
        .collect();
    if !walls_now.is_empty() || world.has_resource::<WallContacts>() {
        world.insert_resource(WallContacts(walls_now.keys().copied().collect()));
    }
    let impacts = wp_physics::contact::take_impacts(world);
    if new.is_empty() && new_walls.is_empty() {
        return;
    }
    let velocity_at = |world: &World, e: Entity, p: Vec2| {
        let pos = world.get::<Pose>(e).map(|p| p.position).unwrap_or_default();
        let v = world.get::<Velocity>(e).map(|v| *v).unwrap_or_default();
        let r = p - pos;
        v.linear + Vec2::new(-v.angular * r.y, v.angular * r.x)
    };
    let corners = |e: Entity| bodies.iter().find(|b| b.0 == e).map(|b| b.1);
    let mut hits = Vec::new();
    for &(a, b) in &new {
        let (Some(ca), Some(cb)) = (corners(a), corners(b)) else {
            continue;
        };
        let contact = wp_physics::contact::box_contact(&ca, &cb);
        let solved = impacts.pairs.get(&(a, b)).copied();
        let point = solved.map(|(_, p)| p).or(contact.map(|c| c.point));
        // Closing speed at first contact (from the solver), else now.
        let kmh = match (solved, contact) {
            (Some((closing, _)), _) => closing.max(0.0) * 3.6,
            (None, Some(c)) => {
                (velocity_at(world, a, c.point) - velocity_at(world, b, c.point))
                    .dot(c.normal)
                    .abs()
                    * 3.6
            }
            (None, None) => 0.0,
        };
        let mass = |e: Entity| world.get::<RigidBody>(e).map_or(1000.0, |b| b.mass);
        let (ma, mb) = (mass(a), mass(b));
        hits.push((a, b.to_string(), kmh, point, mb / (ma + mb)));
        hits.push((b, a.to_string(), kmh, point, ma / (ma + mb)));
    }
    // A building does not give: the body takes all of the crush.
    for (e, wall) in new_walls {
        let Some(corners) = corners(e) else {
            continue;
        };
        let contact = wp_physics::contact::box_contact(&wall, &corners);
        let solved = impacts.walls.get(&e).copied();
        let point = solved.map(|(_, p)| p).or(contact.map(|c| c.point));
        let kmh = match (solved, contact) {
            (Some((closing, _)), _) => closing.max(0.0) * 3.6,
            (None, Some(c)) => velocity_at(world, e, c.point).dot(c.normal).abs() * 3.6,
            (None, None) => 0.0,
        };
        hits.push((e, BUILDING.to_string(), kmh, point, 1.0));
    }
    for &(e, _, kmh, point, share) in &hits {
        let depth = dent_depth_m(kmh, share);
        let (Some(point), Some(pose)) = (point, world.get::<Pose>(e).map(|p| *p)) else {
            continue;
        };
        if depth <= 0.0 {
            continue;
        }
        let local = pose.to_local(point - pose.position);
        let mut damage = world.get::<Damage>(e).map(|d| d.clone()).unwrap_or(Damage {
            dents: Vec::new(),
            health_pct: 100.0,
        });
        damage.dents.push(Dent {
            x: local.x,
            y: local.y,
            depth_m: depth,
        });
        if damage.dents.len() > MAX_DENTS {
            damage.dents.remove(0);
        }
        damage.health_pct = (damage.health_pct - depth * 130.0).max(0.0);
        world.insert(e, damage);
    }
    if let Some(mut events) = world.get_resource_mut::<Events<VehicleEvent>>() {
        // One event per vehicle, so each side can see it was hit.
        for (e, other, kmh, _, _) in hits {
            events.send(VehicleEvent::Collision {
                entity: e.to_string(),
                other,
                impact_kmh: kmh,
            });
        }
    }
}

/// Bodies closer than this count as touching, m.
const TOUCH_M: f32 = 0.03;

fn ordered(a: Entity, b: Entity) -> (Entity, Entity) {
    if a <= b { (a, b) } else { (b, a) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, heading_deg: f32) -> [Vec2; 4] {
        let fp = Footprint {
            front: 2.0,
            rear: -2.0,
            width: 1.8,
        };
        fp.corners(&Pose {
            position: Vec2::new(x, y),
            heading: heading_deg.to_radians(),
        })
    }

    #[test]
    fn separating_axes_find_contact() {
        assert!(
            overlap(&rect(0.0, 0.0, 0.0), &rect(3.9, 0.0, 0.0)),
            "nose to tail, touching"
        );
        assert!(!overlap(&rect(0.0, 0.0, 0.0), &rect(4.1, 0.0, 0.0)));
        assert!(
            !overlap(&rect(0.0, 0.0, 0.0), &rect(0.0, 1.9, 0.0)),
            "side by side, apart"
        );
        // Diagonal neighbours whose bounding boxes overlap but bodies do not.
        assert!(!overlap(&rect(0.0, 0.0, 45.0), &rect(2.6, 2.6, -45.0)));
        assert!(
            overlap(&rect(0.0, 0.0, 90.0), &rect(1.0, 1.0, 0.0)),
            "a T-bone"
        );
    }
}

#[cfg(test)]
mod damage_tests {
    use super::*;
    use crate::{VehicleInput, spawn_vehicle};

    fn car() -> VehicleDef {
        wp_data::parse_ron(
            "hatchback.vehicle.ron",
            include_str!("../../../examples/sandbox/vehicles/hatchback.vehicle.ron"),
        )
        .unwrap()
    }

    /// Two cars driving at each other at `kmh` each: their health after.
    fn head_on(kmh: f32) -> (f32, f32, usize) {
        let mut app = wp_app::App::new();
        app.add_plugin(wp_physics::PhysicsPlugin)
            .add_plugin(crate::VehiclePlugin);
        app.finish().unwrap();
        let def = car();
        let a = spawn_vehicle(&mut app.world, &def, Vec2::new(-12.0, 0.0), 0.0);
        let b = spawn_vehicle(
            &mut app.world,
            &def,
            Vec2::new(12.0, 0.0),
            std::f32::consts::PI,
        );
        for e in [a, b] {
            crate::set_rolling_speed(&mut app.world, e, kmh / 3.6);
            *app.world.get_mut::<VehicleInput>(e).unwrap() = VehicleInput {
                throttle: 0.2,
                ..Default::default()
            };
        }
        app.run_ticks(180);
        let health = |e| app.world.get::<Damage>(e).map_or(100.0, |d| d.health_pct);
        let dents = app.world.get::<Damage>(a).map_or(0, |d| d.dents.len());
        (health(a), health(b), dents)
    }

    #[test]
    fn crashes_dent_both_cars_by_speed() {
        let (a, b, dents) = head_on(30.0);
        assert!(dents >= 1);
        assert!((a - b).abs() < 1.0, "equal cars, equal damage: {a} vs {b}");
        assert!(a < 75.0 && a > 0.0, "a 60 km/h closing crash hurts: {a}");
        let (slow, _, _) = head_on(2.0);
        assert_eq!(slow, 100.0, "a bump at walking pace leaves no dent");
        assert!(dent_depth_m(80.0, 0.5) > dent_depth_m(40.0, 0.5));
        assert!(
            dent_depth_m(40.0, 0.9) > dent_depth_m(40.0, 0.1),
            "the lighter body takes more"
        );
    }
}
