//! Drawing pedestrians: simple standing figures (legs, body, head), lying
//! flat when knocked down.

#[allow(unused_imports)]
use super::*;

pub(super) fn pedestrian_items(
    items: &mut Vec<DrawItem>,
    shadows: &mut Vec<BlobShadow>,
    pose: &Pose,
    down: bool,
    tint: Vec3,
) {
    let mut base =
        Mat4::from_translation(pose.position.extend(0.0)) * Mat4::from_rotation_z(pose.heading);
    if down {
        // Lying on the road: tipped over onto its back.
        base = base
            * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.15))
            * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2);
    }
    let mut box_at = |min: Vec3, max: Vec3, color: Vec3| {
        items.push(DrawItem {
            mesh: MeshId::Cube,
            transform: base
                * Mat4::from_translation((min + max) * 0.5)
                * Mat4::from_scale(max - min),
            color,
            roughness: 0.85,
            metallic: 0.0,
            emissive: 0.0,
            mover: 0,
        });
    };
    let legs = Vec3::new(0.08, 0.09, 0.14);
    let skin = Vec3::new(0.55, 0.36, 0.25);
    box_at(
        Vec3::new(-0.1, -0.17, 0.0),
        Vec3::new(0.1, 0.17, 0.85),
        legs,
    );
    box_at(
        Vec3::new(-0.12, -0.22, 0.85),
        Vec3::new(0.12, 0.22, 1.45),
        tint,
    );
    box_at(
        Vec3::new(-0.1, -0.09, 1.47),
        Vec3::new(0.1, 0.09, 1.72),
        skin,
    );
    shadows.push(BlobShadow {
        center: pose.position,
        half_extents: Vec2::new(0.35, 0.35),
        heading: pose.heading,
        strength: 0.45,
        softness: 0.25,
    });
}
