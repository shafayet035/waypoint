//! Drawing bus stops: a sign on a pole at the kerb and a shelter behind it.

#[allow(unused_imports)]
use super::*;
use wp_road::RoadNetwork;

pub(super) fn stop_items(
    items: &mut Vec<DrawItem>,
    shadows: &mut Vec<BlobShadow>,
    net: &RoadNetwork,
) {
    for st in &net.stops {
        let heading = st.direction.y.atan2(st.direction.x);
        let out = (st.kerb - st.position).normalize_or_zero();
        let base = Mat4::from_translation(st.kerb.extend(0.0)) * Mat4::from_rotation_z(heading);
        let mut box_at = |min: Vec3, max: Vec3, color: Vec3, rough: f32, emissive: f32| {
            items.push(DrawItem {
                mesh: MeshId::Cube,
                transform: base
                    * Mat4::from_translation((min + max) * 0.5)
                    * Mat4::from_scale(max - min),
                color,
                roughness: rough,
                metallic: 0.2,
                emissive,
                mover: 0,
            });
        };
        // Which local side is away from the road: +y if `out` is to the left of travel.
        let away: f32 = if Vec2::new(-st.direction.y, st.direction.x).dot(out) > 0.0 {
            1.0
        } else {
            -1.0
        };
        let steel = Vec3::new(0.25, 0.27, 0.3);
        // Sign: pole and a blue plate with a bright band.
        box_at(
            Vec3::new(4.0, -0.05, 0.0),
            Vec3::new(4.1, 0.05, 2.6),
            steel,
            0.5,
            0.0,
        );
        box_at(
            Vec3::new(3.75, -0.03, 2.2),
            Vec3::new(4.35, 0.03, 2.8),
            Vec3::new(0.02, 0.12, 0.55),
            0.4,
            0.0,
        );
        box_at(
            Vec3::new(3.8, -0.035, 2.55),
            Vec3::new(4.3, 0.035, 2.7),
            Vec3::new(0.9, 0.75, 0.1),
            0.4,
            0.3,
        );
        // Shelter: roof, back panel and two posts, a little away from the kerb.
        let (y0, y1) = if away > 0.0 { (0.4, 1.8) } else { (-1.8, -0.4) };
        let back = if away > 0.0 {
            (1.65, 1.8)
        } else {
            (-1.8, -1.65)
        };
        box_at(
            Vec3::new(-2.0, y0, 2.35),
            Vec3::new(2.0, y1, 2.45),
            steel,
            0.4,
            0.0,
        );
        box_at(
            Vec3::new(-2.0, back.0, 0.1),
            Vec3::new(2.0, back.1, 2.35),
            Vec3::new(0.55, 0.62, 0.66),
            0.08,
            0.0,
        );
        for x in [-1.95, 1.85] {
            box_at(
                Vec3::new(x, y0, 0.0),
                Vec3::new(x + 0.1, y0 + 0.1, 2.35),
                steel,
                0.5,
                0.0,
            );
        }
        // Bench.
        box_at(
            Vec3::new(-1.4, back.0 - 0.45 * away.max(0.0), 0.45),
            Vec3::new(1.4, back.0 + 0.45 * (-away).max(0.0), 0.5),
            Vec3::new(0.35, 0.22, 0.12),
            0.8,
            0.0,
        );
        shadows.push(BlobShadow {
            center: st.kerb + out * 1.1,
            half_extents: Vec2::new(2.1, 0.8),
            heading,
            strength: 0.5,
            softness: 0.3,
        });
    }
}
