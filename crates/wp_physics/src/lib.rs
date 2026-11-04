//! Rigid bodies on the ground plane.
//!
//! M0 scope: planar bodies (position, heading, linear and angular velocity)
//! integrated with semi-implicit Euler at the physics rate. Forces come from
//! other plugins (vehicles) through [`RigidBody::apply_force`].
//!
//! Coordinates: `x` east, `y` north, metres. Heading is radians,
//! counter-clockwise from +x. Body frame: `x` forward, `y` left.

pub mod contact;

pub use contact::{Collider, Contact, box_contact, solve_contacts};
use glam::Vec2;
use serde::{Deserialize, Serialize};
use wp_app::{App, Plugin};
use wp_core::SimTime;
use wp_ecs::{Component, Entity, Stage, World};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub position: Vec2,
    pub heading: f32,
}

impl Component for Pose {
    const NAME: &'static str = "Pose";
}

impl Pose {
    pub fn forward(&self) -> Vec2 {
        Vec2::from_angle(self.heading)
    }
    /// World-frame vector → body frame.
    pub fn to_local(&self, v: Vec2) -> Vec2 {
        Vec2::from_angle(-self.heading).rotate(v)
    }
    /// Body-frame vector → world frame.
    pub fn to_world(&self, v: Vec2) -> Vec2 {
        Vec2::from_angle(self.heading).rotate(v)
    }
    /// Heading in degrees, wrapped to (-180, 180].
    pub fn heading_deg(&self) -> f32 {
        let d = self.heading.to_degrees().rem_euclid(360.0);
        if d > 180.0 { d - 360.0 } else { d }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Velocity {
    /// World frame, m/s.
    pub linear: Vec2,
    /// rad/s, counter-clockwise positive.
    pub angular: f32,
}

impl Component for Velocity {
    const NAME: &'static str = "Velocity";
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigidBody {
    pub mass: f32,
    /// Yaw moment of inertia, kg·m².
    pub inertia: f32,
    #[serde(skip)]
    force: Vec2,
    #[serde(skip)]
    torque: f32,
}

impl Component for RigidBody {
    const NAME: &'static str = "RigidBody";
}

impl RigidBody {
    pub fn new(mass: f32, inertia: f32) -> Self {
        Self {
            mass,
            inertia,
            force: Vec2::ZERO,
            torque: 0.0,
        }
    }
    /// Add a world-frame force through the centre of mass for this substep.
    pub fn apply_force(&mut self, f: Vec2) {
        self.force += f;
    }
    pub fn apply_torque(&mut self, t: f32) {
        self.torque += t;
    }
}

/// Gravitational acceleration, m/s².
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gravity(pub f32);

impl Default for Gravity {
    fn default() -> Self {
        Gravity(9.81)
    }
}

/// Ground surface properties for the flat plane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ground {
    /// Multiplies tyre friction: 1.0 dry asphalt, ~0.7 wet, ~0.3 snow.
    pub friction_scale: f32,
}

impl Default for Ground {
    fn default() -> Self {
        Ground {
            friction_scale: 1.0,
        }
    }
}

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn name(&self) -> &'static str {
        "physics"
    }
    fn build(&self, app: &mut App) {
        let w = &mut app.world;
        w.register::<Pose>();
        w.register::<Velocity>();
        w.register::<RigidBody>();
        // Keep values a scene may already have set; register them for snapshots.
        let gravity = w.remove_resource::<Gravity>().unwrap_or_default();
        w.insert_reflected_resource("Gravity", gravity);
        let ground = w.remove_resource::<Ground>().unwrap_or_default();
        w.insert_reflected_resource("Ground", ground);
        w.register::<PinJoint>();
        w.register::<Collider>();
        // Force producers (vehicles, trailers) run before "integrate_velocities".
        app.add_system(Stage::Physics, "integrate_velocities", integrate_velocities)
            .reads::<RigidBody>()
            .writes::<Velocity>();
        app.add_system(Stage::Physics, "solve_joints", solve_joints)
            .after("integrate_velocities")
            .reads::<PinJoint>()
            .reads::<Pose>()
            .reads::<RigidBody>()
            .writes::<Velocity>();
        app.add_system(Stage::Physics, "solve_contacts", solve_contacts)
            .after("solve_joints")
            .reads::<Collider>()
            .reads::<RigidBody>()
            .writes::<Velocity>()
            .writes::<Pose>();
        app.add_system(Stage::Physics, "integrate_bodies", integrate_bodies)
            .after("solve_contacts")
            .writes::<Pose>()
            .writes::<RigidBody>();
    }
}

/// Semi-implicit Euler, step 1: velocities from this substep's forces.
pub fn integrate_velocities(world: &mut World) {
    let dt = world.resource::<SimTime>().physics_dt();
    world
        .query::<(&mut Velocity, &RigidBody)>()
        .for_each(|_, (vel, body)| {
            vel.linear += body.force / body.mass * dt;
            vel.angular += body.torque / body.inertia * dt;
        });
}

/// Semi-implicit Euler, step 3: positions from the constrained velocities.
pub fn integrate_bodies(world: &mut World) {
    let dt = world.resource::<SimTime>().physics_dt();
    world
        .query::<(&mut Pose, &Velocity, &mut RigidBody)>()
        .for_each(|_, (pose, vel, body)| {
            pose.position += vel.linear * dt;
            pose.heading += vel.angular * dt;
            body.force = Vec2::ZERO;
            body.torque = 0.0;
        });
}

/// A pin (revolute) joint on the ground plane: a point on this body stays on a
/// point of `other`, while both may rotate freely. Used for fifth wheels and
/// drawbars. Stored on the second body.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinJoint {
    pub other: Entity,
    /// Joint point in this body's frame, m from its centre of mass.
    pub local_anchor: Vec2,
    /// Joint point in `other`'s frame, m from its centre of mass.
    pub other_anchor: Vec2,
}

impl Component for PinJoint {
    const NAME: &'static str = "PinJoint";
}

/// Sequential-impulse iterations per substep. Joint chains (multiple
/// trailers) converge with a handful at 480 Hz.
const JOINT_ITERATIONS: usize = 8;
/// Share of position drift corrected per substep (Baumgarte stabilisation).
const JOINT_BAUMGARTE: f32 = 0.2;

pub(crate) fn cross_sv(w: f32, r: Vec2) -> Vec2 {
    Vec2::new(-w * r.y, w * r.x)
}

pub(crate) fn cross_vv(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

/// Remove relative velocity at every pin joint (Box2D-style revolute joint,
/// point constraint only). Deterministic: joints are visited in storage order.
pub fn solve_joints(world: &mut World) {
    let dt = world.resource::<SimTime>().physics_dt();
    let joints = world.query::<&PinJoint>().entities();
    if joints.is_empty() {
        return;
    }
    for _ in 0..JOINT_ITERATIONS {
        for &b in &joints {
            let Some(j) = world.get::<PinJoint>(b).map(|j| *j) else {
                continue;
            };
            let a = j.other;
            let (Some(pa), Some(pb), Some(ba), Some(bb)) = (
                world.get::<Pose>(a).map(|p| *p),
                world.get::<Pose>(b).map(|p| *p),
                world.get::<RigidBody>(a).map(|r| *r),
                world.get::<RigidBody>(b).map(|r| *r),
            ) else {
                continue;
            };
            let (Some(va), Some(vb)) = (
                world.get::<Velocity>(a).map(|v| *v),
                world.get::<Velocity>(b).map(|v| *v),
            ) else {
                continue;
            };
            let ra = pa.to_world(j.other_anchor);
            let rb = pb.to_world(j.local_anchor);
            let (ima, imb, iia, iib) = (
                1.0 / ba.mass,
                1.0 / bb.mass,
                1.0 / ba.inertia,
                1.0 / bb.inertia,
            );
            let k11 = ima + imb + iia * ra.y * ra.y + iib * rb.y * rb.y;
            let k12 = -iia * ra.x * ra.y - iib * rb.x * rb.y;
            let k22 = ima + imb + iia * ra.x * ra.x + iib * rb.x * rb.x;
            let det = k11 * k22 - k12 * k12;
            if det.abs() < 1e-12 {
                continue;
            }
            let cdot = vb.linear + cross_sv(vb.angular, rb) - va.linear - cross_sv(va.angular, ra);
            let c = (pb.position + rb) - (pa.position + ra);
            let rhs = -(cdot + c * (JOINT_BAUMGARTE / dt));
            let p = Vec2::new(k22 * rhs.x - k12 * rhs.y, -k12 * rhs.x + k11 * rhs.y) / det;
            if let Some(mut v) = world.get_mut::<Velocity>(a) {
                v.linear -= p * ima;
                v.angular -= cross_vv(ra, p) * iia;
            }
            if let Some(mut v) = world.get_mut::<Velocity>(b) {
                v.linear += p * imb;
                v.angular += cross_vv(rb, p) * iib;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_force_matches_kinematics() {
        let mut app = App::new();
        app.add_plugin(PhysicsPlugin);
        app.add_system(Stage::Physics, "push", |w| {
            w.query::<&mut RigidBody>()
                .for_each(|_, b| b.apply_force(Vec2::new(1000.0, 0.0)))
        })
        .before("integrate_velocities");
        app.finish().unwrap();
        let e = app.world.spawn();
        app.world.insert(e, Pose::default());
        app.world.insert(e, Velocity::default());
        app.world.insert(e, RigidBody::new(1000.0, 1000.0));
        app.run_ticks(60); // 1 s at 1 m/s²
        let v = app.world.get::<Velocity>(e).unwrap().linear.x;
        let x = app.world.get::<Pose>(e).unwrap().position.x;
        assert!((v - 1.0).abs() < 1e-4, "v = {v}");
        assert!((x - 0.5).abs() < 0.01, "x = {x}");
    }

    #[test]
    fn pin_joint_keeps_bodies_together() {
        let mut app = App::new();
        app.add_plugin(PhysicsPlugin);
        // Pull the front body sideways; the joint must drag the rear one along.
        app.add_system(Stage::Physics, "pull", |w| {
            w.query::<&mut RigidBody>().for_each(|_, b| {
                if b.mass < 2000.0 {
                    b.apply_force(Vec2::new(5000.0, 3000.0));
                }
            })
        })
        .before("integrate_velocities");
        app.finish().unwrap();
        let a = app.world.spawn();
        app.world.insert(
            a,
            Pose {
                position: Vec2::new(2.0, 0.0),
                heading: 0.0,
            },
        );
        app.world.insert(a, Velocity::default());
        app.world.insert(a, RigidBody::new(1000.0, 1500.0));
        let b = app.world.spawn();
        app.world.insert(
            b,
            Pose {
                position: Vec2::new(-4.0, 0.0),
                heading: 0.0,
            },
        );
        app.world.insert(b, Velocity::default());
        app.world.insert(b, RigidBody::new(5000.0, 20000.0));
        // Joint at x = 0: 2 m behind a's centre, 4 m ahead of b's.
        app.world.insert(
            b,
            PinJoint {
                other: a,
                local_anchor: Vec2::new(4.0, 0.0),
                other_anchor: Vec2::new(-2.0, 0.0),
            },
        );
        app.run_ticks(120);
        let (pa, pb) = (
            *app.world.get::<Pose>(a).unwrap(),
            *app.world.get::<Pose>(b).unwrap(),
        );
        let gap = (pa.position + pa.to_world(Vec2::new(-2.0, 0.0)))
            - (pb.position + pb.to_world(Vec2::new(4.0, 0.0)));
        assert!(gap.length() < 0.01, "joint drifted {gap}");
        // 5 kN on 6 t for 2 s: about 1.7 m forward for the pair.
        assert!(
            pb.position.x > -2.6,
            "rear body was not dragged: {:?}",
            pb.position
        );
        assert!(
            pa.heading.abs() > 0.01 || pb.heading.abs() > 0.01,
            "a sideways pull should articulate"
        );
    }

    #[test]
    fn frames_round_trip() {
        let p = Pose {
            position: Vec2::ZERO,
            heading: 0.7,
        };
        let v = Vec2::new(3.0, -2.0);
        assert!((p.to_world(p.to_local(v)) - v).length() < 1e-5);
        assert!((p.to_local(p.forward()) - Vec2::X).length() < 1e-6);
    }
}
