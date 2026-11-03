//! Rigid bodies on the ground plane.
//!
//! M0 scope: planar bodies (position, heading, linear and angular velocity)
//! integrated with semi-implicit Euler at the physics rate. Forces come from
//! other plugins (vehicles) through [`RigidBody::apply_force`].
//!
//! Coordinates: `x` east, `y` north, metres. Heading is radians,
//! counter-clockwise from +x. Body frame: `x` forward, `y` left.

use glam::Vec2;
use serde::{Deserialize, Serialize};
use wp_app::{App, Plugin};
use wp_core::SimTime;
use wp_ecs::{Component, Stage, World};

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
        if !w.has_resource::<Gravity>() {
            w.insert_resource(Gravity::default());
        }
        if !w.has_resource::<Ground>() {
            w.insert_resource(Ground::default());
        }
        app.add_system(Stage::Physics, "integrate_bodies", integrate_bodies)
            .reads::<RigidBody>()
            .writes::<Pose>()
            .writes::<Velocity>()
            .writes::<RigidBody>();
    }
}

/// Semi-implicit Euler: velocity first, then position with the new velocity.
pub fn integrate_bodies(world: &mut World) {
    let dt = world.resource::<SimTime>().physics_dt();
    world
        .query::<(&mut Pose, &mut Velocity, &mut RigidBody)>()
        .for_each(|_, (pose, vel, body)| {
            vel.linear += body.force / body.mass * dt;
            vel.angular += body.torque / body.inertia * dt;
            pose.position += vel.linear * dt;
            pose.heading += vel.angular * dt;
            body.force = Vec2::ZERO;
            body.torque = 0.0;
        });
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
        .before("integrate_bodies");
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
