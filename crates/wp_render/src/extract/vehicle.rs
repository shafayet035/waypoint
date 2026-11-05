//! Drawing vehicles from their definitions: car, bus and truck-cab bodies, wheels, lights.

#[allow(unused_imports)]
use super::*;

/// Body dimensions derived from a vehicle definition until vehicles ship meshes.
#[derive(Clone, Copy, Debug)]
pub struct BodyShape {
    pub front: f32,
    pub rear: f32,
    pub width: f32,
    pub height: f32,
}

pub fn body_shape(def: &VehicleDef) -> BodyShape {
    let fp = wp_vehicle::vehicle_footprint(def);
    let height = match &def.body {
        Some(b) => b.height_m,
        None => (def.cg_height_m * 2.6).max(def.wheel.radius_m * 3.2),
    };
    BodyShape {
        front: fp.front,
        rear: fp.rear,
        width: fp.width,
        height,
    }
}

pub(super) fn palette(i: u32) -> Vec3 {
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
pub(super) fn vehicle_items(
    items: &mut Vec<DrawItem>,
    shadows: &mut Vec<BlobShadow>,
    def: &VehicleDef,
    pose: &Pose,
    veh: &Vehicle,
    paint: Vec3,
    braking: bool,
    model: Option<(u32, &wp_asset::Model)>,
    dented: Option<u32>,
    lights: Option<&mut Vec<PointLight>>,
) {
    let night = lights.is_some();
    let head_glow = if night { 5.0 } else { 0.6 };
    let tail_glow = match (braking, night) {
        (true, _) => 6.0,
        (false, true) => 2.5,
        (false, false) => 0.8,
    };
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
            mover: 0,
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
    if let Some((index, model)) = model {
        // The model's origin is on the ground midway between the axles.
        let at = body * Mat4::from_translation(Vec3::new((a - b) * 0.5, 0.0, 0.0));
        for (k, part) in model.parts.iter().enumerate() {
            let m = &part.material;
            let (color, emissive) = match m.role {
                wp_asset::Role::Paint => (paint, m.emissive),
                wp_asset::Role::BrakeLight => (
                    Vec3::from(m.base_color),
                    if braking || night {
                        tail_glow
                    } else {
                        m.emissive.max(0.8)
                    },
                ),
                wp_asset::Role::HeadLight if night => (Vec3::from(m.base_color), head_glow),
                _ => (Vec3::from(m.base_color), m.emissive),
            };
            items.push(DrawItem {
                mesh: match dented {
                    Some(slot) => MeshId::Deformed {
                        slot,
                        part: k as u32,
                    },
                    None => MeshId::Model {
                        model: index,
                        part: k as u32,
                        lod: 0,
                    },
                },
                transform: at,
                color,
                roughness: m.roughness,
                metallic: m.metallic,
                emissive,
                mover: 0,
            });
        }
    } else if style == BodyStyle::TruckCab {
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
                head_glow,
            );
            box_at(
                Vec3::new(shape.rear - 0.01, side * 1.0 - 0.12, r * 1.25),
                Vec3::new(shape.rear + 0.02, side * 1.0 + 0.12, r * 1.25 + 0.12),
                Vec3::new(0.7, 0.02, 0.02),
                0.2,
                0.0,
                tail_glow,
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
                head_glow,
            );
            box_at(
                Vec3::new(shape.rear - 0.01, y0 - 0.15, lamp_z - 0.05),
                Vec3::new(shape.rear + 0.02, y0 + 0.15, lamp_z + 0.05),
                Vec3::new(0.7, 0.02, 0.02),
                0.2,
                0.0,
                tail_glow,
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
            mover: 0,
        });
    }

    if let Some(lights) = lights {
        // Two dipped beams angled a little down, and the tail lamps.
        let lamp_z = (sill + belt) * 0.5;
        let beam = body
            .transform_vector3(Vec3::new(1.0, 0.0, -0.07))
            .normalize();
        for side in [-1.0, 1.0] {
            let y = side * (hw - 0.3);
            lights.push(PointLight {
                position: body.transform_point3(Vec3::new(shape.front + 0.1, y, lamp_z)),
                color: Vec3::new(1.0, 0.95, 0.85) * 160.0,
                range: 60.0,
                direction: beam,
                cone_cos: 24f32.to_radians().cos(),
            });
            lights.push(PointLight {
                position: body.transform_point3(Vec3::new(shape.rear - 0.3, y, lamp_z)),
                color: Vec3::new(1.0, 0.04, 0.02) * if braking { 3.0 } else { 0.8 },
                range: 5.0,
                direction: Vec3::Z,
                cone_cos: -2.0,
            });
        }
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

/// Bend a model around crash dents. Dents are in the vehicle's pose frame;
/// `shift` is how far the model's origin (axle midpoint) is ahead of it.
/// Each dent pushes the body in, away from the face it hit, with a smooth
/// falloff that widens with depth; normals are rebuilt so the crumple shows.
pub(super) fn deform(
    entity: u32,
    model_index: u32,
    model: &wp_asset::Model,
    dents: &[wp_vehicle::Dent],
    shift: f32,
) -> DeformedModel {
    let mut key: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |x: u32| {
        key ^= x as u64;
        key = key.wrapping_mul(0x0100_0000_01b3);
    };
    mix(entity);
    mix(model_index);
    let cx = (model.min.x + model.max.x) * 0.5;
    let half_len = ((model.max.x - model.min.x) * 0.5).max(0.1);
    let half_w = ((model.max.y - model.min.y) * 0.5).max(0.1);
    let pushes: Vec<(Vec2, Vec2, f32, f32)> = dents
        .iter()
        .map(|d| {
            mix(d.x.to_bits());
            mix(d.y.to_bits());
            mix(d.depth_m.to_bits());
            let at = Vec2::new(d.x - shift, d.y);
            // Hit on a side or an end?
            let inward = if (at.y / half_w).abs() > ((at.x - cx) / half_len).abs() {
                Vec2::new(0.0, -at.y.signum())
            } else {
                Vec2::new(-(at.x - cx).signum(), 0.0)
            };
            (at, inward, d.depth_m, 0.45 + d.depth_m * 1.8)
        })
        .collect();
    let parts = model
        .parts
        .iter()
        .map(|p| {
            let mut part = p.clone();
            for v in &mut part.positions {
                let mut xy = Vec2::new(v[0], v[1]);
                let start = xy;
                for (at, inward, depth, radius) in &pushes {
                    let d = (start - *at).length();
                    if d < *radius {
                        let k = 1.0 - (d / radius) * (d / radius);
                        xy += *inward * depth * k * k;
                    }
                }
                v[0] = xy.x;
                v[1] = xy.y;
            }
            rebuild_normals(&mut part);
            part
        })
        .collect();
    DeformedModel { key, parts }
}

/// Flat normals for unshared triangles, area-weighted ones for shared vertices.
fn rebuild_normals(part: &mut wp_asset::Part) {
    let p = |i: u32| Vec3::from(part.positions[i as usize]);
    let mut normals = vec![Vec3::ZERO; part.positions.len()];
    for t in part.indices.chunks(3) {
        let face = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        for &i in t {
            normals[i as usize] += face;
        }
    }
    for (out, n) in part.normals.iter_mut().zip(normals) {
        *out = n.normalize_or(Vec3::from(*out)).to_array();
    }
}

/// The vehicle's body frame: position and heading, tipping about the outer
/// wheels, then roll and pitch on the springs. Body-frame x is forward from
/// the centre of mass, y left, z up from the ground.
pub fn body_matrix(def: &VehicleDef, pose: &Pose, veh: &Vehicle) -> Mat4 {
    let half_track = def.track_width_m / 2.0;
    let d = if veh.tip_dir >= 0.0 { 1.0 } else { -1.0 };
    let base = Mat4::from_translation(pose.position.extend(0.0))
        * Mat4::from_rotation_z(pose.heading)
        * Mat4::from_translation(Vec3::new(0.0, -d * half_track, 0.0))
        * Mat4::from_rotation_x(d * veh.tip_angle)
        * Mat4::from_translation(Vec3::new(0.0, d * half_track, 0.0));
    let pivot = Vec3::new(0.0, 0.0, def.suspension.roll_center_height_m);
    base * Mat4::from_translation(pivot)
        * Mat4::from_rotation_x(veh.roll)
        * Mat4::from_rotation_y(veh.pitch)
        * Mat4::from_translation(-pivot)
}

/// The driver's eye in the body frame and the cab camera's field of view.
/// `left_seat`: the driver sits left of the centreline (right-hand traffic).
pub fn cab_eye(def: &VehicleDef, left_seat: bool) -> (Vec3, f32) {
    let side = if left_seat { 1.0 } else { -1.0 };
    if let Some(c) = &def.cab {
        return (
            Vec3::new(c.eye_m.0, side * c.eye_m.1.abs(), c.eye_m.2),
            c.fov_deg,
        );
    }
    let shape = body_shape(def);
    let style = match def.body.as_ref().map(|b| b.style).unwrap_or_default() {
        BodyStyle::Auto if shape.height > 2.2 => BodyStyle::Bus,
        BodyStyle::Auto => BodyStyle::Car,
        s => s,
    };
    let (back, y) = match style {
        BodyStyle::Bus => (1.15, shape.width * 0.28),
        BodyStyle::TruckCab => (1.25, shape.width * 0.25),
        _ => (1.95, shape.width * 0.21),
    };
    (
        Vec3::new(shape.front - back, side * y, shape.height * 0.78),
        62.0,
    )
}

/// What the driver sees of the cab: dashboard with lit gauges, steering wheel
/// (turning with the front wheels), windscreen pillars and roof edge.
pub(super) fn cab_items(
    items: &mut Vec<DrawItem>,
    def: &VehicleDef,
    body: Mat4,
    eye: Vec3,
    steer_angle: f32,
    night: bool,
) {
    let shape = body_shape(def);
    let hw = shape.width / 2.0 - 0.06;
    let dark = Vec3::new(0.025, 0.025, 0.028);
    let trim = Vec3::new(0.06, 0.06, 0.065);
    let mut part = |min: Vec3, max: Vec3, color: Vec3, rough: f32, emissive: f32| {
        items.push(DrawItem {
            mesh: MeshId::Cube,
            transform: body
                * Mat4::from_translation((min + max) * 0.5)
                * Mat4::from_scale(max - min),
            color,
            roughness: rough,
            metallic: 0.0,
            emissive,
            mover: 0,
        });
    };
    let dash_x = eye.x + 0.5;
    let dash_top = eye.z - 0.33;
    // Dashboard from the driver's knees to the foot of the windscreen.
    part(
        Vec3::new(dash_x, -hw, dash_top - 0.45),
        Vec3::new(dash_x + 1.0, hw, dash_top),
        dark,
        0.8,
        0.0,
    );
    // Instrument binnacle and its gauges (lit at night).
    part(
        Vec3::new(dash_x + 0.08, eye.y - 0.22, dash_top),
        Vec3::new(dash_x + 0.3, eye.y + 0.22, eye.z - 0.24),
        dark,
        0.8,
        0.0,
    );
    for dy in [-0.08, 0.08] {
        part(
            Vec3::new(dash_x + 0.075, eye.y + dy - 0.045, dash_top + 0.015),
            Vec3::new(dash_x + 0.08, eye.y + dy + 0.045, eye.z - 0.265),
            Vec3::new(0.35, 0.4, 0.45),
            0.3,
            if night { 1.4 } else { 0.1 },
        );
    }
    // Windscreen pillars and the roof edge above the screen.
    let pillar_x = dash_x + 0.55;
    for y in [-hw + 0.02, hw - 0.1] {
        part(
            Vec3::new(pillar_x, y, dash_top),
            Vec3::new(pillar_x + 0.08, y + 0.08, eye.z + 0.32),
            trim,
            0.7,
            0.0,
        );
    }
    part(
        Vec3::new(eye.x - 0.2, -hw, eye.z + 0.28),
        Vec3::new(pillar_x + 0.08, hw, eye.z + 0.36),
        trim,
        0.9,
        0.0,
    );
    // Steering wheel: a ring facing the driver, tilted back, turning about
    // its column (about 15 turns of the wheel per turn of the road wheels).
    let centre = Vec3::new(eye.x + 0.42, eye.y, eye.z - 0.4);
    let spin = Quat::from_rotation_y(-steer_angle * 15.0);
    // The column leans back towards the driver.
    let face = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_x(0.45);
    items.push(DrawItem {
        mesh: MeshId::Wheel,
        transform: body
            * Mat4::from_scale_rotation_translation(
                Vec3::new(0.19, 0.035, 0.19),
                face * spin,
                centre,
            ),
        color: Vec3::new(0.09, 0.09, 0.095),
        roughness: 0.6,
        metallic: 0.2,
        emissive: 0.0,
        mover: 0,
    });
}
