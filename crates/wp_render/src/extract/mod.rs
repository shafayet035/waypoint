//! Build a [`FrameView`] from the simulation. Rendering only reads the world;
//! it never changes simulation state (design principle 3).

mod camera;
mod dock;
mod pedestrian;
mod signal;
mod stop;
mod trailer;
mod vehicle;

pub use camera::{Camera, CameraMode, ChaseSmoothing, Environment};
use dock::dock_items;
use pedestrian::pedestrian_items;
use signal::signal_items;
use stop::stop_items;
use trailer::trailer_items;
pub use vehicle::{BodyShape, body_shape};
use vehicle::{palette, vehicle_items};

use glam::{Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use wp_ecs::{Entity, World};
use wp_physics::Pose;
use wp_vehicle::{
    BodyStyle, Trailer, TrailerDef, TrailerLibrary, Vehicle, VehicleDef, VehicleInput,
    VehicleLibrary,
};
use wp_world::Player;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshId {
    Cube,
    Wheel,
}

#[derive(Clone, Copy, Debug)]
pub struct DrawItem {
    pub mesh: MeshId,
    pub transform: Mat4,
    /// Linear RGB.
    pub color: Vec3,
    pub roughness: f32,
    pub metallic: f32,
    pub emissive: f32,
}

/// A soft rectangular shadow on the ground (cheap enough for every tier).
#[derive(Clone, Copy, Debug)]
pub struct BlobShadow {
    pub center: Vec2,
    pub half_extents: Vec2,
    pub heading: f32,
    pub strength: f32,
    pub softness: f32,
}

#[derive(Clone, Debug)]
pub struct FrameView {
    pub camera: Camera,
    pub environment: Environment,
    pub items: Vec<DrawItem>,
    pub shadows: Vec<BlobShadow>,
    /// The scene's road network; the renderer builds and caches its meshes.
    pub roads: Option<std::sync::Arc<wp_road::RoadNetwork>>,
    /// How wet the road is, 0–1 (darker, glossier asphalt).
    pub wetness: f32,
}

/// The entity the camera follows: the player, else the first vehicle.
pub fn focus_entity(world: &World) -> Option<Entity> {
    world
        .query::<&Player>()
        .entities()
        .first()
        .copied()
        .or_else(|| world.query::<&Vehicle>().entities().first().copied())
}

/// Snapshot the world for drawing.
pub fn extract(world: &World, mode: CameraMode) -> FrameView {
    extract_with(world, mode, None)
}

/// Like [`extract`], but the chase camera looks along `chase_heading` (rad)
/// instead of the vehicle's exact heading: a window smooths it over frames
/// (see [`ChaseSmoothing`]) so the view trails small yaw motions.
pub fn extract_with(world: &World, mode: CameraMode, chase_heading: Option<f32>) -> FrameView {
    let mut items = Vec::new();
    let mut shadows = Vec::new();
    let library = world.get_resource::<VehicleLibrary>();
    let players = world.query::<&Player>().entities();

    world
        .query::<(&Pose, &Vehicle)>()
        .for_each(|e, (pose, veh)| {
            let Some(def) = library.as_ref().and_then(|l| l.0.get(&veh.model)) else {
                return;
            };
            let braking = world.get::<VehicleInput>(e).is_some_and(|i| i.brake > 0.05);
            let paint = if players.contains(&e) {
                Vec3::new(0.62, 0.035, 0.03)
            } else {
                palette(e.index())
            };
            vehicle_items(&mut items, &mut shadows, def, pose, veh, paint, braking);
        });
    world
        .query::<(&Pose, &wp_traffic::pedestrian::Pedestrian)>()
        .for_each(|e, (pose, p)| {
            pedestrian_items(&mut items, &mut shadows, pose, p.down, palette(e.index()));
        });
    if let Some(roads) = world.get_resource::<wp_world::Roads>() {
        let t = world
            .get_resource::<wp_core::SimTime>()
            .map_or(0.0, |t| t.seconds());
        signal_items(&mut items, &roads.0, t);
        stop_items(&mut items, &mut shadows, &roads.0);
    }
    if let Some(docks) = world.get_resource::<wp_world::Docks>() {
        for d in &docks.0 {
            dock_items(&mut items, &mut shadows, d);
        }
    }
    let trailers = world.get_resource::<TrailerLibrary>();
    world
        .query::<(&Pose, &Trailer)>()
        .for_each(|_, (pose, tr)| {
            if let Some(def) = trailers.as_ref().and_then(|l| l.0.get(&tr.model)) {
                trailer_items(&mut items, &mut shadows, def, pose, tr);
            }
        });

    let camera = match focus_entity(world).and_then(|e| world.get::<Pose>(e).map(|p| (e, *p))) {
        Some((e, mut pose)) => {
            if let (CameraMode::Chase, Some(h)) = (mode, chase_heading) {
                pose.heading = h;
            }
            let len = world
                .get::<Vehicle>(e)
                .and_then(|v| {
                    library
                        .as_ref()
                        .and_then(|l| l.0.get(&v.model).map(body_shape))
                })
                .map(|b| b.front - b.rear)
                .unwrap_or(4.2);
            // Frame a towed trailer too.
            let towed = world
                .query::<(&Trailer, &wp_physics::PinJoint)>()
                .entities()
                .into_iter()
                .find(|t| {
                    world
                        .get::<wp_physics::PinJoint>(*t)
                        .is_some_and(|j| j.other == e)
                })
                .and_then(|t| world.get::<Trailer>(t).map(|tr| tr.model.clone()))
                .and_then(|m| {
                    trailers
                        .as_ref()
                        .and_then(|l| l.0.get(&m).map(|d| d.body.length_m))
                })
                .unwrap_or(0.0);
            mode.camera(&pose, len + towed * 0.6)
        }
        None => mode.camera(&Pose::default(), 4.2),
    };
    let weather = world
        .get_resource::<wp_world::Weather>()
        .map(|w| *w)
        .unwrap_or_default();
    if weather.rain > 0.01 {
        let t = world
            .get_resource::<wp_core::SimTime>()
            .map_or(0.0, |t| t.seconds()) as f32;
        let density = world
            .get_resource::<wp_world::Quality>()
            .map_or(1.0, |q| q.0.rain_density());
        rain_items(&mut items, camera.eye, weather.rain * density, t);
    }
    FrameView {
        camera,
        environment: Environment::default().with_rain(weather.rain),
        items,
        shadows,
        roads: world.get_resource::<wp_world::Roads>().map(|r| r.0.clone()),
        wetness: weather.wetness,
    }
}

/// Falling rain: thin bright streaks in a cylinder round the camera, up to
/// 900 in a downpour. Positions come from a hash of the streak's number, so
/// frames are deterministic.
fn rain_items(items: &mut Vec<DrawItem>, eye: Vec3, rain: f32, t: f32) {
    const HEIGHT: f32 = 14.0;
    const FALL_M_S: f32 = 9.0;
    let count = (900.0 * rain) as u32;
    let hash = |i: u32, k: u32| {
        let mut x = i.wrapping_mul(0x9E37_79B9) ^ k.wrapping_mul(0x85EB_CA6B);
        x ^= x >> 15;
        x = x.wrapping_mul(0x2C1B_3C6D);
        x ^= x >> 12;
        (x & 0xFFFF) as f32 / 65535.0
    };
    for i in 0..count {
        let angle = hash(i, 1) * std::f32::consts::TAU;
        let r = 1.5 + 24.0 * hash(i, 2).sqrt();
        let fall = (hash(i, 3) * HEIGHT - t * FALL_M_S).rem_euclid(HEIGHT);
        let p = Vec3::new(
            eye.x + angle.cos() * r,
            eye.y + angle.sin() * r,
            eye.z - 6.0 + fall,
        );
        if p.z < 0.0 {
            continue;
        }
        items.push(DrawItem {
            mesh: MeshId::Cube,
            transform: Mat4::from_translation(p) * Mat4::from_scale(Vec3::new(0.012, 0.012, 0.5)),
            color: Vec3::new(0.62, 0.66, 0.72),
            roughness: 0.2,
            metallic: 0.0,
            emissive: 0.25,
        });
    }
}
