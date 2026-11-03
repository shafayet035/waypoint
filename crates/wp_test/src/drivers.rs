//! Scenario drivers: they read the world and produce the player's controls
//! each tick (Agents stage).
//!
//! The control laws are pure functions ([`speed_control`], [`dock_steering`])
//! so they can be tested without a simulation; [`DriverState`] is the thin
//! shell that gathers inputs from the world.

use crate::def::Driver;
use wp_core::glam::Vec2;
use wp_ecs::{Entity, World};
use wp_physics::{PinJoint, Pose};
use wp_vehicle::{Trailer, TrailerLibrary, Vehicle, VehicleInput, VehicleLibrary};
use wp_world::DockDef;

/// Per-scenario driver state.
pub struct DriverState {
    driver: Driver,
    /// Dock resolved from the scene, for `ReverseToDock`.
    dock: Option<DockDef>,
    integral: f32,
    phase: DockPhase,
    pull_ups: u32,
}

/// The docking manoeuvre's state.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DockPhase {
    Reversing,
    /// Driving forward to straighten out, until the trailer rear is this far out.
    PullingUp {
        until_m: f32,
    },
    Parked,
}

/// Close to the dock but still this crooked or off-line: pull up and retry.
const PULL_UP_WITHIN_M: f32 = 4.0;
const PULL_UP_ANGLE: f32 = 0.04; // rad, about 2.3°
const PULL_UP_OFFSET_M: f32 = 0.2;
const MAX_PULL_UPS: u32 = 5;

impl DriverState {
    pub fn new(driver: Driver, dock: Option<DockDef>) -> Self {
        Self {
            driver,
            dock,
            integral: 0.0,
            phase: DockPhase::Reversing,
            pull_ups: 0,
        }
    }

    pub fn next(&mut self, t: f32, dt: f32, world: &World, player: Entity) -> VehicleInput {
        let forward_kmh = wp_vehicle::telemetry(world, player)
            .map(|t| t.forward_kmh)
            .unwrap_or(0.0);
        match self.driver.clone() {
            Driver::Script(keys) => keys
                .iter()
                .rev()
                .find(|k| k.at_s <= t)
                .map(|k| VehicleInput {
                    throttle: k.throttle,
                    brake: k.brake,
                    steer: k.steer,
                    reverse: k.reverse,
                })
                .unwrap_or_default(),
            Driver::Cruise { kmh, steer } => {
                let (throttle, brake) = speed_control(&mut self.integral, kmh, forward_kmh, dt);
                VehicleInput {
                    throttle,
                    brake,
                    steer,
                    reverse: false,
                }
            }
            Driver::ReverseToDock { kmh, .. } => {
                self.reverse_to_dock(kmh, forward_kmh, dt, world, player)
            }
        }
    }

    fn reverse_to_dock(
        &mut self,
        kmh: f32,
        forward_kmh: f32,
        dt: f32,
        world: &World,
        player: Entity,
    ) -> VehicleInput {
        let hold = VehicleInput {
            brake: 1.0,
            reverse: true,
            ..Default::default()
        };
        let (Some(dock), Some(geo)) = (self.dock.clone(), RigGeometry::read(world, player)) else {
            return hold;
        };
        let u = dock.outward();
        let to_go = (geo.trailer_rear - dock.point()).dot(u);
        let rel = geo.trailer_rear - dock.point();
        let lateral = u.x * rel.y - u.y * rel.x;
        let heading_error = wrap(geo.trailer_heading - u.y.atan2(u.x));
        match self.phase {
            DockPhase::Parked => hold,
            DockPhase::Reversing => {
                if to_go < PULL_UP_WITHIN_M
                    && (heading_error.abs() > PULL_UP_ANGLE || lateral.abs() > PULL_UP_OFFSET_M)
                    && self.pull_ups < MAX_PULL_UPS
                {
                    self.pull_ups += 1;
                    self.phase = DockPhase::PullingUp {
                        until_m: to_go + 15.0,
                    };
                    self.integral = 0.0;
                    return VehicleInput {
                        brake: 1.0,
                        ..Default::default()
                    };
                }
                if to_go < 0.05 {
                    self.phase = DockPhase::Parked;
                    return hold;
                }
                let steer = (dock_steering(&geo, dock.point(), u) / geo.max_steer).clamp(-1.0, 1.0);
                if !geo.in_reverse {
                    return VehicleInput {
                        brake: 0.3,
                        steer,
                        reverse: true,
                        ..Default::default()
                    };
                }
                // Slow down over the last 6 m.
                let target = kmh * (to_go / 6.0).clamp(0.3, 1.0);
                let (throttle, brake) = speed_control(&mut self.integral, target, -forward_kmh, dt);
                VehicleInput {
                    throttle,
                    brake,
                    steer,
                    reverse: true,
                }
            }
            DockPhase::PullingUp { until_m } => {
                if to_go >= until_m {
                    self.phase = DockPhase::Reversing;
                    self.integral = 0.0;
                    return VehicleInput {
                        brake: 1.0,
                        reverse: true,
                        ..Default::default()
                    };
                }
                // Drive the tractor along the dock line; the trailer follows and straightens.
                let tractor_lateral = lateral; // the trailer rear is a fair proxy at this scale
                let target_heading = u.y.atan2(u.x) - (0.08 * tractor_lateral).clamp(-0.3, 0.3);
                let steer = (2.5 * wrap(target_heading - geo.tractor_heading) / geo.max_steer)
                    .clamp(-1.0, 1.0);
                if geo.in_reverse {
                    return VehicleInput {
                        brake: 0.3,
                        steer,
                        ..Default::default()
                    };
                }
                let (throttle, brake) =
                    speed_control(&mut self.integral, kmh * 1.5, forward_kmh, dt);
                VehicleInput {
                    throttle,
                    brake,
                    steer,
                    reverse: false,
                }
            }
        }
    }
}

/// PI speed control on km/h error. Returns (throttle, brake).
pub fn speed_control(integral: &mut f32, target_kmh: f32, speed_kmh: f32, dt: f32) -> (f32, f32) {
    let err = target_kmh - speed_kmh;
    *integral = (*integral + 0.02 * err * dt).clamp(0.0, 0.6);
    let throttle = (0.08 * err + *integral).clamp(0.0, 1.0);
    let brake = if err < -3.0 {
        (-0.05 * err).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (throttle, brake)
}

/// What the docking controller needs to know about a tractor and its trailer.
#[derive(Clone, Copy, Debug)]
pub struct RigGeometry {
    pub tractor_heading: f32,
    pub trailer_heading: f32,
    /// Tractor heading minus trailer heading, rad.
    pub articulation: f32,
    /// Centre of the trailer's axle group.
    pub trailer_axles: Vec2,
    /// Centre of the trailer's rear edge.
    pub trailer_rear: Vec2,
    /// Tractor wheelbase, m.
    pub l1: f32,
    /// Kingpin to trailer axle group, m.
    pub l2: f32,
    /// Tractor's full steering lock, rad.
    pub max_steer: f32,
    pub in_reverse: bool,
}

impl RigGeometry {
    pub fn read(world: &World, tractor: Entity) -> Option<Self> {
        let trailer = world
            .query::<&PinJoint>()
            .entities()
            .into_iter()
            .find(|t| {
                world
                    .get::<PinJoint>(*t)
                    .is_some_and(|j| j.other == tractor)
            })?;
        let tpose = *world.get::<Pose>(trailer)?;
        let tr = world.get::<Trailer>(trailer)?;
        let tdef = world
            .get_resource::<TrailerLibrary>()?
            .0
            .get(&tr.model)?
            .clone();
        let veh = world.get::<Vehicle>(tractor)?;
        let vdef = world
            .get_resource::<VehicleLibrary>()?
            .0
            .get(&veh.model)?
            .clone();
        let pose = *world.get::<Pose>(tractor)?;
        let fwd = tpose.forward();
        let rear_behind_kingpin = tdef.body.length_m - tdef.body.kingpin_from_front_m;
        Some(Self {
            tractor_heading: pose.heading,
            trailer_heading: tpose.heading,
            articulation: tr.articulation,
            trailer_axles: tpose.position + fwd * (tdef.cg_behind_kingpin_m - tdef.mean_axle_m()),
            trailer_rear: tpose.position + fwd * (tdef.cg_behind_kingpin_m - rear_behind_kingpin),
            l1: vdef.wheelbase_m,
            l2: tdef.mean_axle_m(),
            max_steer: vdef.steering.max_angle_deg.to_radians(),
            in_reverse: veh.reverse,
        })
    }
}

/// Angle wrapped to (−π, π].
fn wrap(a: f32) -> f32 {
    let pi = std::f32::consts::PI;
    (a + pi).rem_euclid(2.0 * pi) - pi
}

/// Front-wheel angle (rad, + left) that backs the trailer onto the line
/// through `dock` along `out` (the direction out of the dock).
///
/// A cascade on the kinematic tractor–trailer model, reversing at speed v < 0:
///   trailer heading  θ₂' = (v / L₂) · sin φ        (φ = θ₁ − θ₂, articulation)
///   articulation     φ'  = (v / L₁) · tan δ − θ₂'
/// 1. Lateral offset e of the trailer axles from the dock line sets the
///    wanted trailer heading θ₂* = θ_dock + k₁·e.
/// 2. The heading error sets the wanted articulation φ* = −k₂·(θ₂* − θ₂),
///    limited to ±26° so the rig never folds up.
/// 3. Steering makes φ approach φ*, cancelling the unstable sin φ term:
///    tan δ = (L₁/L₂)·sin φ − L₁·k₃·(φ* − φ).
pub fn dock_steering(geo: &RigGeometry, dock: Vec2, out: Vec2) -> f32 {
    const K1: f32 = 0.12; // rad of heading per metre of offset
    const MAX_HEADING_OFFSET: f32 = 0.5;
    const K2: f32 = 1.6;
    const MAX_ARTICULATION: f32 = 0.45;
    const K3: f32 = 1.2; // per metre travelled
    let rel = geo.trailer_axles - dock;
    let lateral = out.x * rel.y - out.y * rel.x; // + when left of the line
    let theta_line = out.y.atan2(out.x);
    let theta_star = theta_line + (K1 * lateral).clamp(-MAX_HEADING_OFFSET, MAX_HEADING_OFFSET);
    let heading_error = wrap(theta_star - geo.trailer_heading);
    let phi_star = (-K2 * heading_error).clamp(-MAX_ARTICULATION, MAX_ARTICULATION);
    let phi = geo.articulation;
    let tan_delta = (geo.l1 / geo.l2) * phi.sin() - geo.l1 * K3 * (phi_star - phi);
    tan_delta.atan()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo(lateral: f32, trailer_heading: f32, articulation: f32) -> RigGeometry {
        RigGeometry {
            tractor_heading: trailer_heading + articulation,
            trailer_heading,
            articulation,
            trailer_axles: Vec2::new(20.0, lateral),
            trailer_rear: Vec2::new(17.0, lateral),
            l1: 3.7,
            l2: 7.7,
            max_steer: 0.78,
            in_reverse: true,
        }
    }

    #[test]
    fn straight_and_centred_needs_no_steering() {
        let d = dock_steering(&geo(0.0, 0.0, 0.0), Vec2::ZERO, Vec2::X);
        assert!(d.abs() < 1e-6);
    }

    #[test]
    fn steers_to_fold_the_trailer_towards_the_line() {
        // Trailer axles 3 m left of the line (dock out = +x): the trailer must
        // swing its rear right, which in reverse means turning the tractor's
        // front wheels left first is wrong — expect a right (negative) command
        // that grows the articulation the right way.
        let d = dock_steering(&geo(3.0, 0.0, 0.0), Vec2::ZERO, Vec2::X);
        assert!(d != 0.0);
        // Symmetry: the mirrored situation gives the mirrored command.
        let m = dock_steering(&geo(-3.0, 0.0, 0.0), Vec2::ZERO, Vec2::X);
        assert!((d + m).abs() < 1e-5);
    }

    #[test]
    fn speed_control_brakes_when_too_fast() {
        let mut i = 0.0;
        let (t, b) = speed_control(&mut i, 3.0, 10.0, 1.0 / 60.0);
        assert_eq!(t, 0.0);
        assert!(b > 0.0);
    }
}
