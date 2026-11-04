//! Collision events between vehicles and trailers. The physics contact
//! solver (`wp_physics::contact`) makes bodies bump and push each other; this
//! reports, once per tick, which pairs have just started touching, as a
//! `Collision` event to each of them.

use crate::{VehicleDef, VehicleEvent};
use glam::Vec2;
use std::collections::BTreeSet;
use wp_ecs::{Entity, Events, World};
use wp_physics::{Collider, PinJoint, Pose};

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
    if new.is_empty() {
        return;
    }
    if let Some(mut events) = world.get_resource_mut::<Events<VehicleEvent>>() {
        for (a, b) in new {
            // One event per vehicle, so each side can see it was hit.
            for (e, other) in [(a, b), (b, a)] {
                events.send(VehicleEvent::Collision {
                    entity: e.to_string(),
                    other: other.to_string(),
                });
            }
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
