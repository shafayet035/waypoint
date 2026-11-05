//! Drawing semi-trailers: body, deck, landing legs, axles and lights.

#[allow(unused_imports)]
use super::*;

/// A box body on a deck, with each axle's wheels and brake lights.
pub(super) fn trailer_items(
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
            mover: 0,
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
                mover: 0,
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
