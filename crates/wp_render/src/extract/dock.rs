//! Drawing loading docks: warehouse wall, door, bumpers and guide lines.

#[allow(unused_imports)]
use super::*;

/// A loading dock: a warehouse wall with a door and bumpers behind the dock
/// point, and painted guide lines out into the yard.
pub(super) fn dock_items(
    items: &mut Vec<DrawItem>,
    shadows: &mut Vec<BlobShadow>,
    d: &wp_world::DockDef,
) {
    let heading = d.heading_deg.to_radians();
    let base = Mat4::from_translation(d.point().extend(0.0)) * Mat4::from_rotation_z(heading);
    let mut box_at = |min: Vec3, max: Vec3, color: Vec3, rough: f32| {
        items.push(DrawItem {
            mesh: MeshId::Cube,
            transform: base
                * Mat4::from_translation((min + max) * 0.5)
                * Mat4::from_scale(max - min),
            color,
            roughness: rough,
            metallic: 0.0,
            emissive: 0.0,
        });
    };
    let half = d.width_m / 2.0;
    let wall = Vec3::new(0.42, 0.42, 0.4);
    // Building behind the dock line (local −x), with a raised dock floor.
    box_at(
        Vec3::new(-10.0, -half - 4.0, 0.0),
        Vec3::new(-0.35, half + 4.0, 6.0),
        wall,
        0.9,
    );
    box_at(
        Vec3::new(-0.35, -half - 4.0, 0.0),
        Vec3::new(0.0, half + 4.0, 1.25),
        Vec3::new(0.3, 0.3, 0.3),
        0.9,
    );
    // Door opening and the two rubber bumpers.
    box_at(
        Vec3::new(-0.37, -half + 0.3, 1.25),
        Vec3::new(-0.34, half - 0.3, 4.2),
        Vec3::new(0.05, 0.05, 0.06),
        0.6,
    );
    for y in [-half + 0.5, half - 0.5] {
        box_at(
            Vec3::new(-0.05, y - 0.15, 0.8),
            Vec3::new(0.12, y + 0.15, 1.15),
            Vec3::new(0.02, 0.02, 0.02),
            0.9,
        );
    }
    // Yellow guide lines along the bay.
    let paint = Vec3::new(0.8, 0.6, 0.05);
    for y in [-half, half] {
        box_at(
            Vec3::new(0.0, y - 0.06, 0.0),
            Vec3::new(22.0, y + 0.06, 0.012),
            paint,
            0.8,
        );
    }
    shadows.push(BlobShadow {
        center: d.point() - d.outward() * 5.2,
        half_extents: Vec2::new(5.0, half + 4.0),
        heading,
        strength: 0.6,
        softness: 1.2,
    });
}
