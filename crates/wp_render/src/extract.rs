//! Build a [`FrameView`] from the simulation. Rendering only reads the world;
//! it never changes simulation state (design principle 3).

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

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    /// World direction that appears upwards on screen.
    pub up: Vec3,
    pub fov_y_deg: f32,
}

impl Camera {
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        // wgpu clip space matches DirectX: y up, depth 0..1.
        let view = glam::camera::rh::view::look_at_mat4(self.eye, self.target, self.up);
        let proj = glam::camera::rh::proj::directx::perspective(
            self.fov_y_deg.to_radians(),
            aspect,
            0.1,
            3000.0,
        );
        proj * view
    }
}

/// Lighting and atmosphere. Defaults are a clear early afternoon.
#[derive(Clone, Copy, Debug)]
pub struct Environment {
    /// Direction towards the sun.
    pub sun_dir: Vec3,
    pub sun_intensity: f32,
    pub sun_color: Vec3,
    pub sky_zenith: Vec3,
    pub sky_horizon: Vec3,
    pub ground_base: Vec3,
    pub ground_line: Vec3,
    pub fog_density: f32,
    pub exposure: f32,
}

impl Default for Environment {
    fn default() -> Self {
        let elevation = 42f32.to_radians();
        let azimuth = 215f32.to_radians();
        Self {
            sun_dir: Vec3::new(
                azimuth.cos() * elevation.cos(),
                azimuth.sin() * elevation.cos(),
                elevation.sin(),
            ),
            sun_intensity: 2.9,
            sun_color: Vec3::new(1.0, 0.95, 0.88),
            sky_zenith: Vec3::new(0.16, 0.34, 0.78),
            sky_horizon: Vec3::new(0.62, 0.72, 0.86),
            ground_base: Vec3::new(0.055, 0.057, 0.062),
            ground_line: Vec3::new(0.5, 0.5, 0.46),
            fog_density: 0.0012,
            exposure: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrameView {
    pub camera: Camera,
    pub environment: Environment,
    pub items: Vec<DrawItem>,
    pub shadows: Vec<BlobShadow>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraMode {
    /// Behind and above the player, looking ahead.
    #[default]
    Chase,
    /// Straight down, north up; higher for longer vehicles.
    Top,
    /// Level with the player, from its left.
    Side,
    /// Front three-quarter view, for screenshots of the vehicle.
    Showcase,
}

impl CameraMode {
    pub const ALL: [(&'static str, CameraMode); 4] = [
        ("chase", CameraMode::Chase),
        ("top", CameraMode::Top),
        ("side", CameraMode::Side),
        ("showcase", CameraMode::Showcase),
    ];

    pub fn parse(s: &str) -> Option<CameraMode> {
        Self::ALL.iter().find(|(n, _)| *n == s).map(|(_, m)| *m)
    }

    pub fn next(self) -> CameraMode {
        let i = Self::ALL.iter().position(|(_, m)| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()].1
    }

    /// Camera for a subject at `pose`, `length` metres long.
    pub fn camera(self, pose: &Pose, length: f32) -> Camera {
        let pos = pose.position.extend(0.0);
        let fwd = pose.forward().extend(0.0);
        let left = Vec3::Z.cross(fwd);
        let s = (length / 4.2).max(1.0);
        let (eye, target) = match self {
            CameraMode::Chase => (
                pos - fwd * 7.5 * s + Vec3::Z * 2.8 * s,
                pos + fwd * 2.5 * s + Vec3::Z * 0.9 * s,
            ),
            CameraMode::Top => (pos + Vec3::Z * (18.0 + 2.2 * length), pos),
            CameraMode::Side => (
                pos + left * 9.0 * s + Vec3::Z * 1.4 * s,
                pos + Vec3::Z * 0.6 * s,
            ),
            CameraMode::Showcase => (
                pos + fwd * 5.2 * s + left * 3.6 * s + Vec3::Z * 1.7 * s,
                pos + Vec3::Z * 0.55 * s - fwd * 0.3 * s,
            ),
        };
        let up = if self == CameraMode::Top {
            Vec3::Y
        } else {
            Vec3::Z
        };
        Camera {
            eye,
            target,
            up,
            fov_y_deg: 55.0,
        }
    }
}

/// Body dimensions derived from a vehicle definition until vehicles ship meshes.
#[derive(Clone, Copy, Debug)]
pub struct BodyShape {
    pub front: f32,
    pub rear: f32,
    pub width: f32,
    pub height: f32,
}

pub fn body_shape(def: &VehicleDef) -> BodyShape {
    if let Some(b) = &def.body {
        let front = def.cg_to_front_axle_m + b.front_overhang_m;
        return BodyShape {
            front,
            rear: front - b.length_m,
            width: b.width_m,
            height: b.height_m,
        };
    }
    let r = def.wheel.radius_m;
    BodyShape {
        front: def.cg_to_front_axle_m + r * 1.6 + 0.2,
        rear: -(def.rear_axle_from_cg() + r * 1.5 + 0.1),
        width: def.track_width_m + 0.24,
        height: (def.cg_height_m * 2.6).max(r * 3.2),
    }
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
    let trailers = world.get_resource::<TrailerLibrary>();
    world
        .query::<(&Pose, &Trailer)>()
        .for_each(|_, (pose, tr)| {
            if let Some(def) = trailers.as_ref().and_then(|l| l.0.get(&tr.model)) {
                trailer_items(&mut items, &mut shadows, def, pose, tr);
            }
        });

    let camera = match focus_entity(world).and_then(|e| world.get::<Pose>(e).map(|p| (e, *p))) {
        Some((e, pose)) => {
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
    FrameView {
        camera,
        environment: Environment::default(),
        items,
        shadows,
    }
}

/// A box body on a deck, with each axle's wheels and brake lights.
fn trailer_items(
    items: &mut Vec<DrawItem>,
    shadows: &mut Vec<BlobShadow>,
    def: &TrailerDef,
    pose: &Pose,
    tr: &Trailer,
) {
    let base =
        Mat4::from_translation(pose.position.extend(0.0)) * Mat4::from_rotation_z(pose.heading);
    let r = def.wheel.radius_m;
    let front = def.cg_behind_kingpin_m + def.body.kingpin_from_front_m;
    let rear = front - def.body.length_m;
    let hw = def.body.width_m / 2.0;
    let deck = 1.15;
    let mut box_at = |min: Vec3, max: Vec3, color: Vec3, rough: f32, emissive: f32| {
        items.push(DrawItem {
            mesh: MeshId::Cube,
            transform: base
                * Mat4::from_translation((min + max) * 0.5)
                * Mat4::from_scale(max - min),
            color,
            roughness: rough,
            metallic: 0.0,
            emissive,
        });
    };
    let curtain = Vec3::new(0.55, 0.57, 0.6);
    let frame = Vec3::new(0.03, 0.03, 0.035);
    box_at(
        Vec3::new(rear, -hw, deck),
        Vec3::new(front, hw, def.body.height_m),
        curtain,
        0.7,
        0.0,
    );
    box_at(
        Vec3::new(rear, -hw - 0.01, deck - 0.12),
        Vec3::new(front, hw + 0.01, deck + 0.05),
        frame,
        0.6,
        0.0,
    );
    box_at(
        Vec3::new(rear, -hw - 0.01, def.body.height_m - 0.08),
        Vec3::new(front, hw + 0.01, def.body.height_m + 0.01),
        frame,
        0.6,
        0.0,
    );
    // Landing legs and rear bumper.
    let leg_x = def.cg_behind_kingpin_m - 2.2;
    for y in [-0.9, 0.9] {
        box_at(
            Vec3::new(leg_x - 0.08, y - 0.08, 0.35),
            Vec3::new(leg_x + 0.08, y + 0.08, deck),
            frame,
            0.6,
            0.0,
        );
    }
    box_at(
        Vec3::new(rear - 0.02, -hw + 0.2, r * 0.7),
        Vec3::new(rear + 0.15, hw - 0.2, r * 0.95),
        frame,
        0.6,
        0.0,
    );
    for side in [-1.0, 1.0] {
        box_at(
            Vec3::new(rear - 0.02, side * (hw - 0.3) - 0.15, deck - 0.1),
            Vec3::new(rear + 0.01, side * (hw - 0.3) + 0.15, deck + 0.02),
            Vec3::new(0.7, 0.02, 0.02),
            0.2,
            if tr.braking { 6.0 } else { 0.8 },
        );
    }
    for (axle, dist) in def.axles_behind_kingpin_m.iter().enumerate() {
        let x = def.cg_behind_kingpin_m - dist;
        for side in 0..2 {
            let y = if side == 0 {
                def.track_width_m / 2.0
            } else {
                -def.track_width_m / 2.0
            };
            let angle = tr
                .wheels
                .get(axle * 2 + side)
                .map(|w| w.angle)
                .unwrap_or(0.0);
            items.push(DrawItem {
                mesh: MeshId::Wheel,
                transform: base
                    * Mat4::from_scale_rotation_translation(
                        Vec3::new(r, 0.3, r),
                        Quat::from_rotation_y(angle),
                        Vec3::new(x, y, r),
                    ),
                color: Vec3::new(0.025, 0.025, 0.027),
                roughness: 0.85,
                metallic: 0.0,
                emissive: 0.0,
            });
        }
    }
    shadows.push(BlobShadow {
        center: pose.position + pose.forward() * ((front + rear) * 0.5),
        half_extents: Vec2::new(def.body.length_m * 0.5 + 0.1, hw + 0.05),
        heading: pose.heading,
        strength: 0.85,
        softness: 0.4,
    });
}

fn palette(i: u32) -> Vec3 {
    const COLORS: [[f32; 3]; 6] = [
        [0.05, 0.16, 0.5],
        [0.7, 0.7, 0.72],
        [0.04, 0.3, 0.12],
        [0.75, 0.45, 0.05],
        [0.02, 0.02, 0.025],
        [0.35, 0.05, 0.4],
    ];
    Vec3::from(COLORS[i as usize % COLORS.len()])
}

#[allow(clippy::too_many_arguments)]
fn vehicle_items(
    items: &mut Vec<DrawItem>,
    shadows: &mut Vec<BlobShadow>,
    def: &VehicleDef,
    pose: &Pose,
    veh: &Vehicle,
    paint: Vec3,
    braking: bool,
) {
    let base =
        Mat4::from_translation(pose.position.extend(0.0)) * Mat4::from_rotation_z(pose.heading);
    let shape = body_shape(def);
    let r = def.wheel.radius_m;
    let a = def.cg_to_front_axle_m;
    let b = def.rear_axle_from_cg();
    let half_track = def.track_width_m / 2.0;
    let sill = r * 1.25;
    // Tall vehicles (buses) get a low waistline and a long glasshouse.
    let tall = shape.height > 2.2;
    let belt = sill + (shape.height - sill) * if tall { 0.3 } else { 0.45 };

    // Whole vehicle tipping about its outer wheels, then the body rolling and
    // pitching on its springs about the roll centre.
    let d = if veh.tip_dir >= 0.0 { 1.0 } else { -1.0 };
    let base = base
        * Mat4::from_translation(Vec3::new(0.0, -d * half_track, 0.0))
        * Mat4::from_rotation_x(d * veh.tip_angle)
        * Mat4::from_translation(Vec3::new(0.0, d * half_track, 0.0));
    let pivot = Vec3::new(0.0, 0.0, def.suspension.roll_center_height_m);
    let body = base
        * Mat4::from_translation(pivot)
        * Mat4::from_rotation_x(veh.roll)
        * Mat4::from_rotation_y(veh.pitch)
        * Mat4::from_translation(-pivot);

    let mut box_at = |min: Vec3, max: Vec3, color: Vec3, rough: f32, metal: f32, emissive: f32| {
        let center = (min + max) * 0.5;
        let size = max - min;
        items.push(DrawItem {
            mesh: MeshId::Cube,
            transform: body * Mat4::from_translation(center) * Mat4::from_scale(size),
            color,
            roughness: rough,
            metallic: metal,
            emissive,
        });
    };

    let hw = shape.width / 2.0;
    let trim = Vec3::new(0.02, 0.02, 0.022);
    let glass = Vec3::new(0.015, 0.02, 0.028);
    let style = match def.body.as_ref().map(|b| b.style).unwrap_or_default() {
        BodyStyle::Auto if tall => BodyStyle::Bus,
        BodyStyle::Auto => BodyStyle::Car,
        s => s,
    };
    if style == BodyStyle::TruckCab {
        // Cab over the front axle, then an open chassis with the fifth wheel.
        let cab_rear = shape.front - 2.3;
        let cab_floor = r * 1.6;
        box_at(
            Vec3::new(cab_rear, -hw, cab_floor),
            Vec3::new(shape.front, hw, shape.height * 0.62),
            paint,
            0.32,
            0.0,
            0.0,
        );
        box_at(
            Vec3::new(shape.front - 0.35, -hw + 0.06, shape.height * 0.62),
            Vec3::new(shape.front + 0.005, hw - 0.06, shape.height * 0.82),
            glass,
            0.05,
            0.0,
            0.0,
        );
        box_at(
            Vec3::new(cab_rear, -hw, shape.height * 0.62),
            Vec3::new(shape.front - 0.35, hw, shape.height),
            paint,
            0.32,
            0.0,
            0.0,
        );
        box_at(
            Vec3::new(shape.front - 0.35, -hw, shape.height * 0.82),
            Vec3::new(shape.front, hw, shape.height),
            paint,
            0.32,
            0.0,
            0.0,
        );
        box_at(
            Vec3::new(shape.front - 0.3, -hw, r * 0.6),
            Vec3::new(shape.front + 0.02, hw, cab_floor),
            trim,
            0.6,
            0.2,
            0.0,
        );
        // Chassis rails, from under the cab to the rear.
        for y in [-0.45, 0.45] {
            box_at(
                Vec3::new(shape.rear, y - 0.06, r * 1.25),
                Vec3::new(cab_rear + 0.2, y + 0.06, r * 1.25 + 0.3),
                trim,
                0.7,
                0.0,
                0.0,
            );
        }
        if let Some(h) = &def.hitch {
            let x = -b + h.ahead_of_rear_axle_m;
            box_at(
                Vec3::new(x - 0.5, -0.6, r * 1.25 + 0.3),
                Vec3::new(x + 0.5, 0.6, r * 1.25 + 0.42),
                trim,
                0.4,
                0.6,
                0.0,
            );
        }
        // Mudguards over the drive axle.
        box_at(
            Vec3::new(-b - r * 1.2, -hw, r * 2.05),
            Vec3::new(-b + r * 1.2, hw, r * 2.15),
            trim,
            0.6,
            0.0,
            0.0,
        );
        for side in [-1.0, 1.0] {
            let y0 = side * (hw - 0.25);
            box_at(
                Vec3::new(shape.front - 0.02, y0 - 0.2, cab_floor - 0.25),
                Vec3::new(shape.front + 0.03, y0 + 0.2, cab_floor - 0.1),
                Vec3::new(0.9, 0.9, 0.85),
                0.1,
                0.0,
                0.6,
            );
            box_at(
                Vec3::new(shape.rear - 0.01, side * 1.0 - 0.12, r * 1.25),
                Vec3::new(shape.rear + 0.02, side * 1.0 + 0.12, r * 1.25 + 0.12),
                Vec3::new(0.7, 0.02, 0.02),
                0.2,
                0.0,
                if braking { 6.0 } else { 0.8 },
            );
        }
    } else {
        // Floor between the wheels, lower body, glasshouse and roof.
        box_at(
            Vec3::new(-b + r * 1.05, -half_track + 0.2, r * 0.45),
            Vec3::new(a - r * 1.05, half_track - 0.2, sill),
            trim,
            0.8,
            0.0,
            0.0,
        );
        box_at(
            Vec3::new(shape.rear, -hw, sill),
            Vec3::new(shape.front, hw, belt),
            paint,
            0.32,
            0.0,
            0.0,
        );
        box_at(
            Vec3::new(shape.rear + 0.05, -hw, r * 0.7),
            Vec3::new(shape.rear + 0.3, hw, sill),
            trim,
            0.7,
            0.0,
            0.0,
        );
        box_at(
            Vec3::new(shape.front - 0.3, -hw, r * 0.7),
            Vec3::new(shape.front - 0.05, hw, sill),
            trim,
            0.7,
            0.0,
            0.0,
        );
        let (cabin_front, cabin_rear) = if tall {
            (shape.front - 0.08, shape.rear + 0.2)
        } else {
            (a - r * 1.3, shape.rear + 0.3)
        };
        box_at(
            Vec3::new(cabin_rear, -hw + 0.07, belt),
            Vec3::new(cabin_front, hw - 0.07, shape.height - 0.06),
            glass,
            0.05,
            0.0,
            0.0,
        );
        box_at(
            Vec3::new(cabin_rear + 0.05, -hw + 0.09, shape.height - 0.07),
            Vec3::new(cabin_front - 0.1, hw - 0.09, shape.height),
            paint,
            0.32,
            0.0,
            0.0,
        );
        // Lights.
        let lamp_z = (sill + belt) * 0.5;
        for side in [-1.0, 1.0] {
            let y0 = side * (hw - 0.12);
            box_at(
                Vec3::new(shape.front - 0.02, y0 - 0.17, lamp_z - 0.06),
                Vec3::new(shape.front + 0.01, y0 + 0.17, lamp_z + 0.06),
                Vec3::new(0.9, 0.9, 0.85),
                0.1,
                0.0,
                0.6,
            );
            box_at(
                Vec3::new(shape.rear - 0.01, y0 - 0.15, lamp_z - 0.05),
                Vec3::new(shape.rear + 0.02, y0 + 0.15, lamp_z + 0.05),
                Vec3::new(0.7, 0.02, 0.02),
                0.2,
                0.0,
                if braking { 6.0 } else { 0.8 },
            );
        }
    }

    // Wheels.
    let positions = [
        Vec3::new(a, half_track, r),
        Vec3::new(a, -half_track, r),
        Vec3::new(-b, half_track, r),
        Vec3::new(-b, -half_track, r),
    ];
    for (i, p) in positions.iter().enumerate() {
        let steer = if i < 2 { veh.steer_angle } else { 0.0 };
        let rot = Quat::from_rotation_z(steer) * Quat::from_rotation_y(veh.wheels[i].angle);
        items.push(DrawItem {
            mesh: MeshId::Wheel,
            transform: base * Mat4::from_scale_rotation_translation(Vec3::new(r, 0.22, r), rot, *p),
            color: Vec3::new(0.025, 0.025, 0.027),
            roughness: 0.85,
            metallic: 0.0,
            emissive: 0.0,
        });
    }

    let mid = (shape.front + shape.rear) * 0.5;
    shadows.push(BlobShadow {
        center: pose.position + pose.forward() * mid,
        half_extents: Vec2::new((shape.front - shape.rear) * 0.5 + 0.1, hw + 0.05),
        heading: pose.heading,
        strength: 0.88,
        softness: 0.35,
    });
}
