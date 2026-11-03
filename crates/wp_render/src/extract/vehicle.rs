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
