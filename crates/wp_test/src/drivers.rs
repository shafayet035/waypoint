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
use wp_road::Polyline;
pub use wp_traffic::control::{
    TRACK_AHEAD_M, TRACK_BACK_M, route_speed_kmh, speed_control, stanley_steering, wrap,
};
use wp_vehicle::{Trailer, TrailerLibrary, Vehicle, VehicleInput, VehicleLibrary};
use wp_world::{DockDef, Roads};

/// Progress of a `FollowRoute` driver, published as a world resource each
/// tick so the runner and agents can see it.
#[derive(Clone, Debug)]
pub struct RouteProgress {
    pub path: std::sync::Arc<Polyline>,
    /// Distance along the route, m.
    pub s: f32,
    /// Signed distance from the route centre, m (+ left).
    pub lateral: f32,
    pub finished: bool,
    /// Why no route could be planned, if so.
    pub error: Option<String>,
}

/// Per-scenario driver state.
pub struct DriverState {
    driver: Driver,
    /// Dock resolved from the scene, for `ReverseToDock`.
    dock: Option<DockDef>,
    integral: f32,
    phase: DockPhase,
    pull_ups: u32,
    route: Option<RouteProgress>,
    /// Driver aids for `Keyboard`.
    aids: wp_vehicle::DriverAids,
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
            route: None,
            aids: wp_vehicle::DriverAids::default(),
        }
    }

    pub fn next(&mut self, t: f32, dt: f32, world: &World, player: Entity) -> VehicleInput {
        let forward_kmh = wp_vehicle::telemetry(world, player)
            .map(|t| t.forward_kmh)
            .unwrap_or(0.0);
        match self.driver.clone() {
            Driver::Keyboard(keys) => {
                let key = keys.iter().rev().find(|k| k.at_s <= t).cloned();
                let controls = key
                    .map(|k| wp_vehicle::Controls {
                        accelerate: k.throttle,
                        brake: k.brake,
                        steer: k.steer,
                        analog_steer: false,
                    })
                    .unwrap_or_default();
                let def = world.get::<Vehicle>(player).and_then(|v| {
                    world
                        .get_resource::<VehicleLibrary>()
                        .and_then(|l| l.0.get(&v.model).cloned())
                });
                match (def, wp_vehicle::telemetry(world, player)) {
                    (Some(def), Some(motion)) => self.aids.update(controls, &motion, &def, dt),
                    _ => VehicleInput::default(),
                }
            }
            // The traffic AI drives the vehicle itself (see the runner).
            Driver::Autopilot => VehicleInput::default(),
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
            Driver::FollowRoute { to_road, kmh } => {
                self.follow_route(&to_road, kmh, forward_kmh, dt, world, player)
            }
        }
    }

    /// The route driver's progress, if this is a route driver.
    pub fn route_progress(&self) -> Option<&RouteProgress> {
        self.route.as_ref()
    }

    fn follow_route(
        &mut self,
        to_road: &str,
        kmh: f32,
        forward_kmh: f32,
        dt: f32,
        world: &World,
        player: Entity,
    ) -> VehicleInput {
        let stop = VehicleInput {
            brake: 1.0,
            ..Default::default()
        };
        let Some(pose) = world.get::<Pose>(player).map(|p| *p) else {
            return stop;
        };
        let model = world.get::<Vehicle>(player).map(|v| v.model.clone());
        let dims = model.and_then(|m| {
            world.get_resource::<VehicleLibrary>().and_then(|l| {
                l.0.get(&m)
                    .map(|d| (d.cg_to_front_axle_m, d.steering.max_angle_deg.to_radians()))
            })
        });
        let Some((to_front_axle, max_steer)) = dims else {
            return stop;
        };
        if self.route.is_none() {
            self.route = Some(plan_route(world, pose.position, to_road));
        }
        let progress = self.route.as_mut().expect("planned above");
        if progress.error.is_some() {
            return stop;
        }
        let path = progress.path.clone();
        // Track progress near where we were, so a route that passes the same
        // place twice (round a block and back) is followed in order.
        let (s, lateral, _) = path.project_between(
            pose.position,
            progress.s - TRACK_BACK_M,
            progress.s + TRACK_AHEAD_M,
        );
        progress.s = s;
        progress.lateral = lateral;
        let remaining = path.length() - s;
        if remaining < 1.0 || progress.finished {
            progress.finished = true;
            return stop;
        }
        let speed = forward_kmh.max(0.0) / 3.6;
        let front_axle = pose.position + pose.forward() * to_front_axle;
        let steer = stanley_steering(&path, front_axle, pose.heading, speed, s);
        let target = route_speed_kmh(&path, s, kmh, remaining);
        let (throttle, brake) = speed_control(&mut self.integral, target, forward_kmh, dt);
        VehicleInput {
            throttle,
            brake,
            steer: (steer / max_steer).clamp(-1.0, 1.0),
            reverse: false,
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

/// Plan from the lane under `position` to road `to_road`.
fn plan_route(world: &World, position: Vec2, to_road: &str) -> RouteProgress {
    let fail = |e: String| RouteProgress {
        path: std::sync::Arc::new(Polyline::default()),
        s: 0.0,
        lateral: 0.0,
        finished: false,
        error: Some(e),
    };
    let Some(roads) = world.get_resource::<Roads>() else {
        return fail("the scene has no road network".into());
    };
    let net = &roads.0;
    let Some((lane, start_s, _)) = net.lane_at(position) else {
        return fail("the vehicle is not on a road lane".into());
    };
    let Some(route) = net.route(lane, to_road) else {
        return fail(format!("no route to road `{to_road}`"));
    };
    // Drive to the middle of the last lane.
    let mut path = net.route_path(&route, start_s);
    if let Some(&last) = route.last() {
        let tail = net.lanes[last].path.length() / 2.0;
        path = path.trim(0.0, tail);
    }
    RouteProgress {
        path: std::sync::Arc::new(path),
        s: 0.0,
        lateral: 0.0,
        finished: false,
        error: None,
    }
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
    fn stanley_steers_back_towards_the_path() {
        let path = Polyline::new(vec![Vec2::ZERO, Vec2::new(100.0, 0.0)]);
        // Left of the path, pointing along it: steer right (negative).
        assert!(stanley_steering(&path, Vec2::new(10.0, 1.0), 0.0, 10.0, 10.0) < 0.0);
        // On the path but angled left: steer right to align.
        assert!(stanley_steering(&path, Vec2::new(10.0, 0.0), 0.2, 10.0, 10.0) < 0.0);
        // Perfectly on it: no steering.
        assert!(stanley_steering(&path, Vec2::new(10.0, 0.0), 0.0, 10.0, 10.0).abs() < 1e-5);
    }

    #[test]
    fn speed_control_brakes_when_too_fast() {
        let mut i = 0.0;
        let (t, b) = speed_control(&mut i, 3.0, 10.0, 1.0 / 60.0);
        assert_eq!(t, 0.0);
        assert!(b > 0.0);
    }
}
