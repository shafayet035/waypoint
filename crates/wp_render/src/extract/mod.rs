//! Build a [`FrameView`] from the simulation. Rendering only reads the world;
//! it never changes simulation state (design principle 3).

mod camera;
mod dock;
mod pedestrian;
mod signal;
mod stop;
mod trailer;
mod vehicle;
pub mod windscreen;

pub use camera::{Camera, CameraMode, ChaseSmoothing, Environment};
use dock::dock_items;
use pedestrian::pedestrian_items;
use signal::signal_items;
use stop::stop_items;
use trailer::trailer_items;
pub use vehicle::{BodyShape, body_matrix, body_shape, cab_eye};
use vehicle::{palette, vehicle_items};
pub use windscreen::{Windscreen, WiperSpeed, Wipers};

use glam::{Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use wp_ecs::{Entity, World};
use wp_physics::Pose;
use wp_vehicle::{
    BodyStyle, Trailer, TrailerDef, TrailerLibrary, Vehicle, VehicleDef, VehicleInput,
    VehicleLibrary,
};
use wp_world::Player;

/// What a draw item draws. Items are drawn in this order (cubes, wheels,
/// then model parts by model and part).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MeshId {
    Cube,
    Wheel,
    /// Tree crowns (the renderer swaps in the far versions itself).
    Crown,
    Cone,
    Fronds,
    CrownFar,
    ConeFar,
    /// Part `part` of model `model` in [`FrameView::models`], at level of
    /// detail `lod` (0 = full; the renderer picks it from screen size).
    Model {
        model: u32,
        part: u32,
        lod: u8,
    },
    /// Part `part` of [`FrameView::deformed`]`[slot]` (a crash-damaged model).
    Deformed {
        slot: u32,
        part: u32,
    },
}

/// A model bent by crash damage, for one vehicle. `key` changes whenever
/// the damage does, so the renderer uploads it once per change.
#[derive(Clone, Debug)]
pub struct DeformedModel {
    pub key: u64,
    pub parts: Vec<wp_asset::Part>,
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

/// A local light: headlamps, tail lights, street lamps.
#[derive(Clone, Copy, Debug)]
pub struct PointLight {
    pub position: Vec3,
    /// Linear RGB times intensity.
    pub color: Vec3,
    /// Light reaches zero at this distance, m.
    pub range: f32,
    /// Spot direction (unit); unused for omni lights.
    pub direction: Vec3,
    /// Cosine of the spot's half angle; below −1 for an omni light.
    pub cone_cos: f32,
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
    /// The scene's buildings, if it has any.
    pub scenery: Option<std::sync::Arc<wp_world::scenery::Scenery>>,
    /// How wet the road is, 0–1 (darker, glossier asphalt, puddles,
    /// streaks of light).
    pub wetness: f32,
    /// How hard it rains, 0–1.
    pub rain: f32,
    /// Simulated time, s.
    pub time_s: f32,
    /// Models that [`MeshId::Model`] items refer to; the renderer uploads
    /// them once per library.
    pub models: Option<std::sync::Arc<wp_asset::ModelLibrary>>,
    /// Local lights; the renderer uses the nearest ones its tier allows.
    pub lights: Vec<PointLight>,
    /// Crash-damaged models that [`MeshId::Deformed`] items refer to.
    pub deformed: Vec<DeformedModel>,
    /// The game's art style.
    pub look: wp_world::Look,
    /// Rain on the windscreen (cab view in the rain).
    pub windscreen: Option<Windscreen>,
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
    extract_view(
        world,
        mode,
        &ViewOptions {
            chase_heading,
            ..Default::default()
        },
    )
}

/// Per-frame camera controls from a window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewOptions {
    /// Smoothed heading for the chase camera (see [`ChaseSmoothing`]).
    pub chase_heading: Option<f32>,
    /// Head turn in the cab: yaw (left positive) and pitch (up positive), rad.
    pub look: (f32, f32),
}

/// [`extract`] with window camera controls.
pub fn extract_view(world: &World, mode: CameraMode, view: &ViewOptions) -> FrameView {
    let chase_heading = view.chase_heading;
    let mut items = Vec::new();
    let mut shadows = Vec::new();
    let mut lights = Vec::new();
    let mut deformed: Vec<DeformedModel> = Vec::new();
    let weather = world
        .get_resource::<wp_world::Weather>()
        .map(|w| *w)
        .unwrap_or_default();
    let look = world
        .get_resource::<wp_world::Look>()
        .map(|l| *l)
        .unwrap_or_default();
    let day = Environment::from_look(&look);
    let environment = match world.get_resource::<wp_world::TimeOfDay>() {
        Some(t) => Environment::at_hour_from(day, t.hour),
        None => day,
    }
    .with_rain(weather.rain)
    .with_fog(weather.fog, weather.visibility_m());
    let darkness = environment.darkness();
    // Drivers switch on at dusk, in heavy rain or in fog; street lamps at dusk.
    let headlights = darkness > 0.25 || weather.rain > 0.5 || weather.fog > 0.3;
    let lamps_lit = darkness > 0.4;
    let library = world.get_resource::<VehicleLibrary>();
    let models = world
        .get_resource::<wp_asset::Models>()
        .map(|m| m.0.clone());
    let players = world.query::<&Player>().entities();
    let time_s = world
        .get_resource::<wp_core::SimTime>()
        .map_or(0.0, |t| t.seconds()) as f32;
    let mut windscreen = None;

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
            let model = def
                .model
                .as_ref()
                .and_then(|p| models.as_ref()?.find(p))
                .map(|(i, m)| (i, m.as_ref()));
            let damage = world.get::<wp_vehicle::Damage>(e).map(|d| d.clone());
            let dented = match (model, damage) {
                (Some((index, m)), Some(d)) if !d.dents.is_empty() => {
                    let shift = (def.cg_to_front_axle_m - def.rear_axle_from_cg()) * 0.5;
                    let slot = deformed.len() as u32;
                    deformed.push(vehicle::deform(e.index(), index, m, &d.dents, shift));
                    Some(slot)
                }
                _ => None,
            };
            vehicle_items(
                &mut items,
                &mut shadows,
                def,
                pose,
                veh,
                paint,
                braking,
                model,
                dented,
                headlights.then_some(&mut lights),
            );
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
        lamp_items(
            &mut items,
            &mut lights,
            &roads.0,
            focus_point(world),
            lamps_lit,
        );
    }
    if let Some(scenery) = world.get_resource::<wp_world::scenery::CurrentScenery>() {
        tree_items(&mut items, &scenery.0, focus_point(world));
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
            let cab = (mode == CameraMode::Cab)
                .then(|| {
                    let veh = world.get::<Vehicle>(e)?;
                    let def = library.as_ref()?.0.get(&veh.model)?.clone();
                    Some((def, veh.clone()))
                })
                .flatten();
            match cab {
                Some((def, veh)) => {
                    let left_seat = world
                        .get_resource::<wp_world::Roads>()
                        .is_none_or(|r| r.0.drive_side == wp_road::DriveSide::Right);
                    let (eye, fov) = cab_eye(&def, left_seat);
                    let body = body_matrix(&def, &pose, &veh);
                    vehicle::cab_items(&mut items, &def, body, eye, veh.steer_angle, headlights);
                    let glass = windscreen::glass(body_shape(&def).width / 2.0 - 0.06, eye);
                    let wipers = Wipers::for_glass(
                        glass.1,
                        glass.2,
                        !left_seat,
                        WiperSpeed::for_rain(weather.rain),
                    );
                    windscreen::wiper_items(&mut items, body, glass, &wipers, time_s);
                    if weather.rain > 0.01 {
                        windscreen = Some(Windscreen {
                            origin: body.transform_point3(glass.0),
                            across: body.transform_vector3(Vec3::NEG_Y).normalize(),
                            up: body.transform_vector3(Vec3::Z).normalize(),
                            width_m: glass.1,
                            height_m: glass.2,
                            rain: weather.rain,
                            time_s,
                            wipers,
                        });
                    }
                    let at = body.transform_point3(eye);
                    let (yaw, pitch) = view.look;
                    let gaze = Quat::from_rotation_z(yaw.clamp(-2.4, 2.4))
                        * Quat::from_rotation_y(-pitch.clamp(-0.6, 0.6))
                        * Vec3::new(1.0, 0.0, -0.06);
                    Camera {
                        eye: at,
                        target: at + body.transform_vector3(gaze) * 10.0,
                        up: body.transform_vector3(Vec3::Z),
                        fov_y_deg: fov,
                    }
                }
                None => mode.camera(&pose, len + towed * 0.6),
            }
        }
        None => mode.camera(&Pose::default(), 4.2),
    };
    // Outside cameras stay out of buildings: pull in in front of the first wall.
    let mut camera = camera;
    if matches!(
        mode,
        CameraMode::Chase | CameraMode::Side | CameraMode::Showcase
    ) && let Some(scenery) = world.get_resource::<wp_world::scenery::CurrentScenery>()
        && let Some(t) = scenery.0.first_hit(camera.target, camera.eye)
    {
        let span = camera.eye - camera.target;
        let keep = ((t * span.length() - 0.5) / span.length()).max(0.08);
        camera.eye = camera.target + span * keep;
    }
    if weather.rain > 0.01 {
        let t = time_s;
        let density = world
            .get_resource::<wp_world::Quality>()
            .map_or(1.0, |q| q.0.rain_density());
        rain_items(&mut items, camera.eye, weather.rain * density, t);
    }
    FrameView {
        camera,
        environment,
        items,
        shadows,
        roads: world.get_resource::<wp_world::Roads>().map(|r| r.0.clone()),
        scenery: world
            .get_resource::<wp_world::scenery::CurrentScenery>()
            .map(|s| s.0.clone()),
        wetness: weather.wetness,
        rain: weather.rain,
        time_s,
        models,
        lights,
        deformed,
        look,
        windscreen,
    }
}

/// Where the camera looks: the followed vehicle, else the origin.
fn focus_point(world: &World) -> Vec2 {
    focus_entity(world)
        .and_then(|e| world.get::<Pose>(e).map(|p| p.position))
        .unwrap_or(Vec2::ZERO)
}

/// Lamps drawn within this distance of the focus, m.
const LAMP_DRAW_M: f32 = 250.0;

/// Street lamps: a pole, an arm over the road and a head that glows (and
/// lights the street) when `lit`.
/// Trees within this distance of the focus are drawn, m.
const TREE_DRAW_M: f32 = 450.0;

/// Trees: a trunk (a box) and a crown (lumpy sphere, cone, or a flat palm top).
pub(crate) fn tree_items(
    items: &mut Vec<DrawItem>,
    scenery: &wp_world::scenery::Scenery,
    focus: Vec2,
) {
    use wp_world::scenery::{TRUNK_M, TreeKind};
    let bark = Vec3::new(0.11, 0.075, 0.05);
    let leaves = [
        Vec3::new(0.06, 0.15, 0.035),
        Vec3::new(0.09, 0.18, 0.04),
        Vec3::new(0.05, 0.12, 0.04),
        Vec3::new(0.11, 0.17, 0.045),
    ];
    for t in &scenery.trees {
        if t.position.distance(focus) > TREE_DRAW_M {
            continue;
        }
        let h = t.height_m;
        let vary = 0.88 + (t.seed % 25) as f32 / 100.0;
        let green = leaves[(t.seed as usize >> 2) % leaves.len()];
        let at = t.position.extend(0.0);
        let (trunk_h, trunk_w, crown, crown_at, crown_size) = match t.kind {
            TreeKind::Round => (
                h * 0.45,
                TRUNK_M,
                MeshId::Crown,
                h * 0.64,
                Vec3::new(h * 0.3 * vary, h * 0.3 * vary, h * 0.34),
            ),
            TreeKind::Cone => (
                h * 0.2,
                TRUNK_M * 0.9,
                MeshId::Cone,
                h * 0.12,
                Vec3::new(h * 0.21 * vary, h * 0.21 * vary, h * 0.88),
            ),
            TreeKind::Palm => (
                h * 0.95,
                TRUNK_M * 0.7,
                MeshId::Fronds,
                h * 0.94,
                Vec3::new(h * 0.42 * vary, h * 0.42 * vary, h * 0.42),
            ),
        };
        items.push(DrawItem {
            mesh: MeshId::Cube,
            transform: Mat4::from_translation(at + Vec3::Z * trunk_h * 0.5)
                * Mat4::from_scale(Vec3::new(trunk_w, trunk_w, trunk_h)),
            color: bark,
            roughness: 0.9,
            metallic: 0.0,
            emissive: 0.0,
        });
        items.push(DrawItem {
            mesh: crown,
            transform: Mat4::from_translation(at + Vec3::Z * crown_at)
                * Mat4::from_rotation_z(t.seed as f32 * 0.7)
                * Mat4::from_scale(crown_size),
            color: green,
            roughness: 0.85,
            metallic: 0.0,
            emissive: 0.0,
        });
    }
}

pub(crate) fn lamp_items(
    items: &mut Vec<DrawItem>,
    lights: &mut Vec<PointLight>,
    net: &wp_road::RoadNetwork,
    focus: Vec2,
    lit: bool,
) {
    const HEIGHT: f32 = 7.5;
    const ARM: f32 = 1.6;
    let pole = Vec3::new(0.16, 0.17, 0.18);
    for lamp in &net.lamps {
        if lamp.position.distance(focus) > LAMP_DRAW_M {
            continue;
        }
        let base = Mat4::from_translation(lamp.position.extend(0.0))
            * Mat4::from_rotation_z(lamp.toward_road.y.atan2(lamp.toward_road.x));
        let mut part = |min: Vec3, max: Vec3, color: Vec3, emissive: f32| {
            items.push(DrawItem {
                mesh: MeshId::Cube,
                transform: base
                    * Mat4::from_translation((min + max) * 0.5)
                    * Mat4::from_scale(max - min),
                color,
                roughness: 0.55,
                metallic: 0.4,
                emissive,
            });
        };
        part(
            Vec3::new(-0.08, -0.08, 0.0),
            Vec3::new(0.08, 0.08, HEIGHT),
            pole,
            0.0,
        );
        part(
            Vec3::new(0.0, -0.05, HEIGHT - 0.12),
            Vec3::new(ARM, 0.05, HEIGHT - 0.02),
            pole,
            0.0,
        );
        let warm = Vec3::new(1.0, 0.78, 0.5);
        part(
            Vec3::new(ARM - 0.45, -0.16, HEIGHT - 0.22),
            Vec3::new(ARM + 0.1, 0.16, HEIGHT - 0.08),
            if lit { warm } else { Vec3::new(0.5, 0.5, 0.48) },
            if lit { 6.0 } else { 0.0 },
        );
        if lit {
            let head = lamp.position + lamp.toward_road * (ARM - 0.15);
            lights.push(PointLight {
                position: head.extend(HEIGHT - 0.3),
                color: warm * 22.0,
                range: 20.0,
                direction: Vec3::NEG_Z,
                cone_cos: 65f32.to_radians().cos(),
            });
        }
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
