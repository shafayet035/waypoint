//! Drawing traffic lights: a pole and a three-lamp head for each approach to
//! a signalled junction, on the drivers' side, lit by the current phase.

#[allow(unused_imports)]
use super::*;
use wp_road::{Light, RoadNetwork};

pub(super) fn signal_items(items: &mut Vec<DrawItem>, net: &RoadNetwork, t: f64) {
    let kerb_side = match net.drive_side {
        wp_road::DriveSide::Right => -1.0,
        wp_road::DriveSide::Left => 1.0,
    };
    for j in &net.junctions {
        if !matches!(j.control, wp_road::Control::Signals(_)) {
            continue;
        }
        for &(road, point, away) in &j.arms {
            let Some(light) = j.light(road, t) else {
                continue;
            };
            let g = &net.roads[road];
            // Arriving traffic drives along -away; the pole stands beside the
            // road on the drivers' side, the head facing them.
            let arriving = -away;
            let side = Vec2::new(-arriving.y, arriving.x) * kerb_side;
            let reach = g.left_m.max(g.right_m) + g.def.sidewalk_m.max(0.0) * 0.5 + 0.8;
            let at = point + side * reach;
            let base = Mat4::from_translation(at.extend(0.0))
                * Mat4::from_rotation_z(away.y.atan2(away.x));
            let mut box_at = |min: Vec3, max: Vec3, color: Vec3, emissive: f32| {
                items.push(DrawItem {
                    mesh: MeshId::Cube,
                    transform: base
                        * Mat4::from_translation((min + max) * 0.5)
                        * Mat4::from_scale(max - min),
                    color,
                    roughness: 0.6,
                    metallic: 0.3,
                    emissive,
                });
            };
            let dark = Vec3::new(0.03, 0.03, 0.035);
            box_at(
                Vec3::new(-0.07, -0.07, 0.0),
                Vec3::new(0.07, 0.07, 2.4),
                dark,
                0.0,
            );
            box_at(
                Vec3::new(-0.15, -0.17, 2.3),
                Vec3::new(0.15, 0.17, 3.35),
                dark,
                0.0,
            );
            let lamps = [
                (3.12, Light::Red, Vec3::new(1.0, 0.06, 0.03)),
                (2.83, Light::Amber, Vec3::new(1.0, 0.45, 0.02)),
                (2.54, Light::Green, Vec3::new(0.05, 1.0, 0.25)),
            ];
            for (z, which, colour) in lamps {
                let on = which == light;
                let (c, e) = if on {
                    (colour, 6.0)
                } else {
                    (colour * 0.08, 0.0)
                };
                // On the face towards arriving traffic (local +x).
                box_at(
                    Vec3::new(0.15, -0.1, z - 0.1),
                    Vec3::new(0.19, 0.1, z + 0.1),
                    c,
                    e,
                );
            }
        }
    }
}
