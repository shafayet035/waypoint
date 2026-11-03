//! Build a [`FrameView`] from the simulation. Rendering only reads the world;
//! it never changes simulation state (design principle 3).

use glam::{Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use wp_ecs::{Entity, World};
use wp_physics::Pose;
use wp_vehicle::{Vehicle, VehicleDef, VehicleInput, VehicleLibrary};
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
    /// Straight down from 30 m, north up.
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
            CameraMode::Top => (pos + Vec3::Z * 30.0 * s, pos),
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
            mode.camera(&pose, len)
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
