//! Contacts between bodies on the ground plane: each body's footprint is a
//! box ([`Collider`]); touching boxes push each other apart with impulses
//! (slightly bouncy, with friction between them), so vehicles bump, shove
//! and spin each other instead of passing through. Solved every substep,
//! after joints. Bodies joined by a [`PinJoint`] (a tractor and its trailer)
//! never collide with each other.
//!
//! Buildings are [`Walls`]: fixed, immovable segments (a world resource built
//! from the scene's scenery). Bodies stop against them the same way.

use crate::{PinJoint, Pose, RigidBody, Velocity, cross_sv, cross_vv};
use glam::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use wp_ecs::{Component, Entity, World};

/// A body's footprint: a box along its heading, from `rear` to `front` (m
/// from the centre of mass, front positive) and `half_width` either side.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collider {
    pub front: f32,
    pub rear: f32,
    pub half_width: f32,
}

impl Component for Collider {
    const NAME: &'static str = "Collider";
}

impl Collider {
    /// The four corners at `pose`, going round the box.
    pub fn corners(&self, pose: &Pose) -> [Vec2; 4] {
        let f = pose.forward();
        let l = Vec2::new(-f.y, f.x) * self.half_width;
        let (a, b) = (
            pose.position + f * self.front,
            pose.position + f * self.rear,
        );
        [a + l, a - l, b - l, b + l]
    }

    /// Radius of a circle around the centre of mass that holds the box.
    pub fn radius(&self) -> f32 {
        self.front.abs().max(self.rear.abs()).hypot(self.half_width)
    }
}

/// Where two boxes touch: the normal points from the first box to the second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub normal: Vec2,
    pub depth: f32,
    pub point: Vec2,
}

/// Separating-axis test for two boxes; the contact along the axis of least
/// overlap, at the mean of the corners that are inside the other box.
pub fn box_contact(a: &[Vec2; 4], b: &[Vec2; 4]) -> Option<Contact> {
    let centre = |q: &[Vec2; 4]| (q[0] + q[1] + q[2] + q[3]) / 4.0;
    let (ca, cb) = (centre(a), centre(b));
    let mut best: Option<(f32, Vec2)> = None;
    for q in [a, b] {
        for i in 0..2 {
            let edge = q[i + 1] - q[i];
            let n = Vec2::new(-edge.y, edge.x).normalize_or_zero();
            let span = |r: &[Vec2; 4]| {
                r.iter()
                    .map(|p| p.dot(n))
                    .fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)))
            };
            let ((a0, a1), (b0, b1)) = (span(a), span(b));
            let overlap = a1.min(b1) - a0.max(b0);
            if overlap <= 0.0 {
                return None;
            }
            if best.is_none_or(|(d, _)| overlap < d) {
                best = Some((overlap, n));
            }
        }
    }
    let (depth, mut normal) = best?;
    if (cb - ca).dot(normal) < 0.0 {
        normal = -normal;
    }
    let inside = |p: Vec2, q: &[Vec2; 4]| {
        (0..4).all(|i| {
            (q[(i + 1) % 4] - q[i]).perp_dot(p - q[i]) * (q[1] - q[0]).perp_dot(q[2] - q[0]) >= 0.0
        })
    };
    let pts: Vec<Vec2> = a
        .iter()
        .filter(|p| inside(**p, b))
        .chain(b.iter().filter(|p| inside(**p, a)))
        .copied()
        .collect();
    let point = if pts.is_empty() {
        (ca + cb) / 2.0
    } else {
        pts.iter().copied().sum::<Vec2>() / pts.len() as f32
    };
    Some(Contact {
        normal,
        depth,
        point,
    })
}

/// Bounciness of vehicle-on-vehicle impacts (crumpling bodies absorb most).
const RESTITUTION: f32 = 0.15;
/// Friction between touching bodies.
const BODY_FRICTION: f32 = 0.35;
/// Overlap left alone, and share of the rest removed per substep.
const SLOP_M: f32 = 0.01;
const PUSH_OUT: f32 = 0.4;
const ITERATIONS: usize = 6;

struct Body {
    entity: Entity,
    pose: Pose,
    vel: Velocity,
    inv_mass: f32,
    inv_inertia: f32,
    corners: [Vec2; 4],
    radius: f32,
}

/// Hardest closing speed (m/s) and where, per touching pair (and per body
/// against walls) since the last [`take_impacts`]: the contact solver stops
/// bodies within a substep, so this is the only place the impact speed is
/// still known.
#[derive(Clone, Debug, Default)]
pub struct Impacts {
    pub pairs: std::collections::BTreeMap<(Entity, Entity), (f32, Vec2)>,
    pub walls: std::collections::BTreeMap<Entity, (f32, Vec2)>,
}

/// How far walls reach into the building behind their face, m: thick enough
/// that a fast body cannot pass through within one physics step.
pub const WALL_THICKNESS_M: f32 = 0.5;
/// Grid cell for finding walls near a body, m.
const WALL_CELL_M: f32 = 16.0;

/// Fixed walls (building sides) that bodies cannot pass. Each is a thin box
/// behind a face segment; build with [`Walls::new`] from outlines.
#[derive(Clone, Debug, Default)]
pub struct Walls {
    /// Corners of each wall's box, going round it; the first two are the face.
    pub boxes: Vec<[Vec2; 4]>,
    grid: std::collections::BTreeMap<(i32, i32), Vec<u32>>,
}

impl Walls {
    /// Walls round counter-clockwise outlines (seen from above): each edge
    /// faces outwards and is backed by [`WALL_THICKNESS_M`] of wall.
    pub fn new<'a>(outlines: impl IntoIterator<Item = &'a [Vec2]>) -> Self {
        let mut walls = Walls::default();
        for outline in outlines {
            let n = outline.len();
            for i in 0..n {
                let (a, b) = (outline[i], outline[(i + 1) % n]);
                let len = a.distance(b);
                if len < 0.05 {
                    continue;
                }
                let d = (b - a) / len;
                let inward = Vec2::new(-d.y, d.x) * WALL_THICKNESS_M;
                walls.push([a, b, b + inward, a + inward]);
            }
        }
        walls
    }

    fn push(&mut self, q: [Vec2; 4]) {
        let k = self.boxes.len() as u32;
        let (lo, hi) = bounds(&q);
        for cx in cell(lo.x)..=cell(hi.x) {
            for cy in cell(lo.y)..=cell(hi.y) {
                self.grid.entry((cx, cy)).or_default().push(k);
            }
        }
        self.boxes.push(q);
    }

    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    /// Indices of walls whose cells touch the circle at `p` of radius `r`, in order.
    pub fn near(&self, p: Vec2, r: f32) -> Vec<u32> {
        let mut out = Vec::new();
        for cx in cell(p.x - r)..=cell(p.x + r) {
            for cy in cell(p.y - r)..=cell(p.y + r) {
                if let Some(v) = self.grid.get(&(cx, cy)) {
                    out.extend_from_slice(v);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

fn cell(x: f32) -> i32 {
    (x / WALL_CELL_M).floor() as i32
}

fn bounds(q: &[Vec2; 4]) -> (Vec2, Vec2) {
    let lo = q.iter().copied().fold(Vec2::splat(f32::MAX), Vec2::min);
    let hi = q.iter().copied().fold(Vec2::splat(f32::MIN), Vec2::max);
    (lo, hi)
}

/// The impacts recorded since the last call, emptied.
pub fn take_impacts(world: &mut World) -> Impacts {
    world
        .get_resource_mut::<Impacts>()
        .map(|mut i| std::mem::take(&mut *i))
        .unwrap_or_default()
}

/// Push touching bodies apart (velocity impulses, then a little position),
/// and out of walls.
pub fn solve_contacts(world: &mut World) {
    let mut bodies: Vec<Body> = Vec::new();
    world
        .query::<(&Pose, &Collider, &RigidBody, &Velocity)>()
        .for_each(|e, (pose, c, rb, vel)| {
            bodies.push(Body {
                entity: e,
                pose: *pose,
                vel: *vel,
                inv_mass: 1.0 / rb.mass,
                inv_inertia: 1.0 / rb.inertia,
                corners: c.corners(pose),
                radius: c.radius(),
            });
        });
    if bodies.is_empty() {
        return;
    }
    let joined: BTreeSet<(Entity, Entity)> = world
        .query::<&PinJoint>()
        .entities()
        .into_iter()
        .filter_map(|e| world.get::<PinJoint>(e).map(|j| ordered(e, j.other)))
        .collect();
    let mut contacts: Vec<(usize, usize, Contact, f32, f32, f32)> = Vec::new();
    let mut impacts = Vec::new();
    for i in 0..bodies.len() {
        for j in i + 1..bodies.len() {
            let (a, b) = (&bodies[i], &bodies[j]);
            if a.pose.position.distance(b.pose.position) > a.radius + b.radius
                || joined.contains(&ordered(a.entity, b.entity))
            {
                continue;
            }
            if let Some(c) = box_contact(&a.corners, &b.corners) {
                // Closing speed at first contact sets the bounce.
                let (ra, rb) = (c.point - a.pose.position, c.point - b.pose.position);
                let vrel = b.vel.linear + cross_sv(b.vel.angular, rb)
                    - a.vel.linear
                    - cross_sv(a.vel.angular, ra);
                let vn = vrel.dot(c.normal);
                let bounce = if vn < -0.5 { -RESTITUTION * vn } else { 0.0 };
                impacts.push((ordered(a.entity, b.entity), -vn, c.point));
                contacts.push((i, j, c, bounce, 0.0, 0.0));
            }
        }
    }
    // Walls: (body, contact from the wall to the body, bounce, normal and
    // friction impulse so far).
    let mut wall_contacts: Vec<(usize, Contact, f32, f32, f32)> = Vec::new();
    let mut wall_impacts = Vec::new();
    if let Some(walls) = world.get_resource::<Walls>() {
        for (k, b) in bodies.iter().enumerate() {
            for w in walls.near(b.pose.position, b.radius) {
                let q = &walls.boxes[w as usize];
                if let Some(c) = box_contact(q, &b.corners) {
                    let r = c.point - b.pose.position;
                    let vn = (b.vel.linear + cross_sv(b.vel.angular, r)).dot(c.normal);
                    let bounce = if vn < -0.5 { -RESTITUTION * vn } else { 0.0 };
                    wall_impacts.push((b.entity, -vn, c.point));
                    wall_contacts.push((k, c, bounce, 0.0, 0.0));
                }
            }
        }
    }
    if contacts.is_empty() && wall_contacts.is_empty() {
        return;
    }
    if !world.has_resource::<Impacts>() {
        world.insert_resource(Impacts::default());
    }
    {
        let mut rec = world.resource_mut::<Impacts>();
        for (pair, closing, point) in impacts {
            let e = rec.pairs.entry(pair).or_insert((0.0, point));
            if closing > e.0 {
                *e = (closing, point);
            }
        }
        for (body, closing, point) in wall_impacts {
            let e = rec.walls.entry(body).or_insert((0.0, point));
            if closing > e.0 {
                *e = (closing, point);
            }
        }
    }
    for _ in 0..ITERATIONS {
        for contact in contacts.iter_mut() {
            let (i, j, c, bounce, pn, pt) = *contact;
            let (a, b) = (&bodies[i], &bodies[j]);
            let (ra, rb) = (c.point - a.pose.position, c.point - b.pose.position);
            let n = c.normal;
            let t = Vec2::new(-n.y, n.x);
            let vrel = |a: &Body, b: &Body| {
                b.vel.linear + cross_sv(b.vel.angular, rb)
                    - a.vel.linear
                    - cross_sv(a.vel.angular, ra)
            };
            let mass_along = |d: Vec2| {
                a.inv_mass
                    + b.inv_mass
                    + a.inv_inertia * cross_vv(ra, d).powi(2)
                    + b.inv_inertia * cross_vv(rb, d).powi(2)
            };
            // Normal: stop the bodies closing (and bounce a little).
            let vn = vrel(a, b).dot(n);
            let new_pn = (pn - (vn - bounce) / mass_along(n)).max(0.0);
            let dn = new_pn - pn;
            // Friction: resist sliding along the contact, up to μ·normal.
            let vt = vrel(a, b).dot(t);
            let limit = BODY_FRICTION * new_pn;
            let new_pt = (pt - vt / mass_along(t)).clamp(-limit, limit);
            let dt_ = new_pt - pt;
            let p = n * dn + t * dt_;
            let (ia, ib) = (a.inv_mass, b.inv_mass);
            let (iia, iib) = (a.inv_inertia, b.inv_inertia);
            bodies[i].vel.linear -= p * ia;
            bodies[i].vel.angular -= cross_vv(ra, p) * iia;
            bodies[j].vel.linear += p * ib;
            bodies[j].vel.angular += cross_vv(rb, p) * iib;
            contact.4 = new_pn;
            contact.5 = new_pt;
        }
        for contact in wall_contacts.iter_mut() {
            let (k, c, bounce, pn, pt) = *contact;
            let b = &bodies[k];
            let r = c.point - b.pose.position;
            let n = c.normal;
            let t = Vec2::new(-n.y, n.x);
            let v = b.vel.linear + cross_sv(b.vel.angular, r);
            let mass_along = |d: Vec2| b.inv_mass + b.inv_inertia * cross_vv(r, d).powi(2);
            let new_pn = (pn - (v.dot(n) - bounce) / mass_along(n)).max(0.0);
            let limit = BODY_FRICTION * new_pn;
            let new_pt = (pt - v.dot(t) / mass_along(t)).clamp(-limit, limit);
            let p = n * (new_pn - pn) + t * (new_pt - pt);
            let (im, ii) = (b.inv_mass, b.inv_inertia);
            bodies[k].vel.linear += p * im;
            bodies[k].vel.angular += cross_vv(r, p) * ii;
            contact.3 = new_pn;
            contact.4 = new_pt;
        }
    }
    // Remove most of the overlap, the lighter body moving more.
    for &(i, j, c, ..) in &contacts {
        let push = (c.depth - SLOP_M).max(0.0) * PUSH_OUT;
        let total = bodies[i].inv_mass + bodies[j].inv_mass;
        let (si, sj) = (bodies[i].inv_mass / total, bodies[j].inv_mass / total);
        bodies[i].pose.position -= c.normal * push * si;
        bodies[j].pose.position += c.normal * push * sj;
    }
    // Walls do not move: the body takes all of the push.
    for &(k, c, ..) in &wall_contacts {
        bodies[k].pose.position += c.normal * (c.depth - SLOP_M).max(0.0) * PUSH_OUT;
    }
    let touched: BTreeSet<usize> = contacts
        .iter()
        .flat_map(|c| [c.0, c.1])
        .chain(wall_contacts.iter().map(|c| c.0))
        .collect();
    for k in touched {
        let b = &bodies[k];
        if let Some(mut v) = world.get_mut::<Velocity>(b.entity) {
            *v = b.vel;
        }
        if let Some(mut p) = world.get_mut::<Pose>(b.entity) {
            *p = b.pose;
        }
    }
}

fn ordered(a: Entity, b: Entity) -> (Entity, Entity) {
    if a <= b { (a, b) } else { (b, a) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PhysicsPlugin;
    use wp_app::App;

    fn body(app: &mut App, x: f32, heading: f32, vx: f32, mass: f32) -> Entity {
        let e = app.world.spawn();
        app.world.insert(
            e,
            Pose {
                position: Vec2::new(x, 0.0),
                heading,
            },
        );
        app.world.insert(
            e,
            Velocity {
                linear: Vec2::new(vx, 0.0),
                angular: 0.0,
            },
        );
        app.world.insert(e, RigidBody::new(mass, mass * 1.5));
        app.world.insert(
            e,
            Collider {
                front: 2.0,
                rear: -2.0,
                half_width: 0.9,
            },
        );
        e
    }

    #[test]
    fn head_on_bodies_stop_overlapping_and_keep_momentum() {
        let mut app = App::new();
        app.add_plugin(PhysicsPlugin);
        app.finish().unwrap();
        let a = body(&mut app, -5.0, 0.0, 10.0, 1200.0);
        let b = body(&mut app, 5.0, 0.0, -10.0, 1200.0);
        app.run_ticks(120);
        let (pa, pb) = (
            app.world.get::<Pose>(a).unwrap().position,
            app.world.get::<Pose>(b).unwrap().position,
        );
        let (va, vb) = (
            app.world.get::<Velocity>(a).unwrap().linear,
            app.world.get::<Velocity>(b).unwrap().linear,
        );
        assert!(
            pb.x - pa.x > 3.9,
            "boxes 4 m long must not pass through: {pa} {pb}"
        );
        assert!((va + vb).length() < 0.05, "momentum kept: {va} {vb}");
        assert!(
            va.x < 0.0 && va.x > -2.5,
            "a small bounce, not elastic: {va}"
        );
    }

    #[test]
    fn a_heavy_body_shoves_a_light_one() {
        let mut app = App::new();
        app.add_plugin(PhysicsPlugin);
        app.finish().unwrap();
        let bus = body(&mut app, -5.0, 0.0, 5.0, 15000.0);
        let car = body(&mut app, 0.0, 0.0, 0.0, 1200.0);
        app.run_ticks(60);
        let vc = app.world.get::<Velocity>(car).unwrap().linear.x;
        let vb = app.world.get::<Velocity>(bus).unwrap().linear.x;
        assert!(
            vc > 4.0 && vb > 4.0 && vb < 5.0,
            "car pushed along: car {vc}, bus {vb}"
        );
    }

    #[test]
    fn an_off_centre_hit_spins_the_body() {
        let mut app = App::new();
        app.add_plugin(PhysicsPlugin);
        app.finish().unwrap();
        // A T-bone near the rear of a car lying across the path.
        let hitter = body(&mut app, -6.0, 0.0, 8.0, 1200.0);
        let target = body(&mut app, 0.0, std::f32::consts::FRAC_PI_2, 0.0, 1200.0);
        app.world.get_mut::<Pose>(target).unwrap().position.y = 1.5;
        app.run_ticks(60);
        let w = app.world.get::<Velocity>(target).unwrap().angular;
        assert!(w.abs() > 0.1, "spun: {w} rad/s");
        let _ = hitter;
    }

    #[test]
    fn separating_axes_and_contact_points() {
        let c = Collider {
            front: 2.0,
            rear: -2.0,
            half_width: 0.9,
        };
        let at = |x: f32, y: f32, h: f32| {
            c.corners(&Pose {
                position: Vec2::new(x, y),
                heading: h,
            })
        };
        assert!(box_contact(&at(0.0, 0.0, 0.0), &at(4.1, 0.0, 0.0)).is_none());
        let k = box_contact(&at(0.0, 0.0, 0.0), &at(3.8, 0.0, 0.0)).unwrap();
        assert!((k.depth - 0.2).abs() < 1e-4 && (k.normal - Vec2::X).length() < 1e-4);
        assert!(
            box_contact(&at(0.0, 0.0, 0.8), &at(2.6, 2.6, -0.8)).is_none(),
            "diagonal near miss"
        );
    }
}
