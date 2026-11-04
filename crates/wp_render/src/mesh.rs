//! Built-in procedural meshes. Real vehicle models (glTF) come with the asset
//! pipeline; these keep M0 free of binary assets.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// 0 = instance colour, 1 = hub metal. Used for wheel spokes.
    pub tint: f32,
    /// Texture coordinates (0,0 for untextured meshes); on building walls,
    /// metres along the wall and up it.
    pub uv: [f32; 2],
    /// sRGB colour multiplying the instance colour (white for most meshes);
    /// alpha varies which windows of a building are lit.
    pub color: [u8; 4],
    /// Building facade pattern (see `wp_world::scenery::SceneryMesh`); zero: none.
    pub facade: [u8; 4],
}

impl Default for Vertex {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            tint: 0.0,
            uv: [0.0; 2],
            color: [255; 4],
            facade: [0; 4],
        }
    }
}

pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// Unit cube from −0.5 to 0.5, flat-shaded, counter-clockwise outward faces.
pub fn cube() -> MeshData {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (n, u, v) in faces {
        let base = vertices.len() as u32;
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = [
                0.5 * (n[0] + su * u[0] + sv * v[0]),
                0.5 * (n[1] + su * u[1] + sv * v[1]),
                0.5 * (n[2] + su * u[2] + sv * v[2]),
            ];
            vertices.push(Vertex {
                position: p,
                normal: n,
                tint: 0.0,
                uv: [0.0, 0.0],
                ..Default::default()
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    MeshData { vertices, indices }
}

/// Wheel: a cylinder of radius 1 along the Y axis from −0.5 to 0.5, with a
/// spoked hub on both faces so rotation is visible.
pub fn wheel() -> MeshData {
    const SEG: usize = 30;
    const HUB: f32 = 0.62;
    const SPOKES: usize = 5;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let ang = |i: usize| (i as f32 / SEG as f32) * std::f32::consts::TAU;

    // Tread: smooth normals.
    for i in 0..=SEG {
        let (s, c) = ang(i).sin_cos();
        for y in [-0.5, 0.5] {
            vertices.push(Vertex {
                position: [s, y, c],
                normal: [s, 0.0, c],
                tint: 0.0,
                uv: [0.0, 0.0],
                ..Default::default()
            });
        }
    }
    for i in 0..SEG as u32 {
        let a = i * 2;
        // Outward-facing winding.
        indices.extend_from_slice(&[a, a + 2, a + 1, a + 1, a + 2, a + 3]);
    }

    // Faces: sidewall ring (tyre) and hub sectors (spokes bright, gaps dark).
    for side in [-0.5f32, 0.5] {
        let n = [0.0, side.signum(), 0.0];
        for i in 0..SEG {
            let (s0, c0) = ang(i).sin_cos();
            let (s1, c1) = ang(i + 1).sin_cos();
            let spoke = (i * SPOKES * 2 / SEG).is_multiple_of(2);
            let hub_tint = if spoke { 1.0 } else { 0.25 };
            let base = vertices.len() as u32;
            let ring = [
                [s0 * HUB, side, c0 * HUB],
                [s1 * HUB, side, c1 * HUB],
                [s1, side, c1],
                [s0, side, c0],
            ];
            for p in ring {
                vertices.push(Vertex {
                    position: p,
                    normal: n,
                    tint: 0.0,
                    uv: [0.0, 0.0],
                    ..Default::default()
                });
            }
            let centre = vertices.len() as u32;
            vertices.push(Vertex {
                position: [0.0, side, 0.0],
                normal: n,
                tint: hub_tint,
                uv: [0.0, 0.0],
                ..Default::default()
            });
            vertices.push(Vertex {
                position: ring[0],
                normal: n,
                tint: hub_tint,
                uv: [0.0, 0.0],
                ..Default::default()
            });
            vertices.push(Vertex {
                position: ring[1],
                normal: n,
                tint: hub_tint,
                uv: [0.0, 0.0],
                ..Default::default()
            });
            if side > 0.0 {
                indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
                indices.extend_from_slice(&[centre, centre + 1, centre + 2]);
            } else {
                indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                indices.extend_from_slice(&[centre, centre + 2, centre + 1]);
            }
        }
    }
    MeshData { vertices, indices }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every triangle's winding must agree with its vertex normals (so back-face
    /// culling never hides a visible face).
    fn check_winding(m: &MeshData) {
        for t in m.indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize]);
            let p = |v: Vertex| glam::Vec3::from(v.position);
            let face = (p(b) - p(a)).cross(p(c) - p(a));
            if face.length() < 1e-9 {
                continue;
            }
            let n = glam::Vec3::from(a.normal)
                + glam::Vec3::from(b.normal)
                + glam::Vec3::from(c.normal);
            assert!(face.dot(n) > 0.0, "triangle {t:?} is wound inward");
        }
    }

    #[test]
    fn meshes_face_outward() {
        check_winding(&cube());
        check_winding(&wheel());
    }
}

/// Tree crown: a lumpy sphere of radius about 1 (an icosahedron split
/// `splits` times, each vertex pushed in or out a little), smooth normals.
pub fn crown(splits: u32) -> MeshData {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut pos: Vec<glam::Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| glam::Vec3::new(x, y, z).normalize())
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..splits {
        let mut mid = std::collections::BTreeMap::new();
        let mut next = Vec::with_capacity(tris.len() * 4);
        let mut half = |a: u32, b: u32, pos: &mut Vec<glam::Vec3>| -> u32 {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                pos.push(((pos[a as usize] + pos[b as usize]) * 0.5).normalize());
                pos.len() as u32 - 1
            })
        };
        for [a, b, c] in tris {
            let (ab, bc, ca) = (
                half(a, b, &mut pos),
                half(b, c, &mut pos),
                half(c, a, &mut pos),
            );
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    // Lumps: a fixed pattern, so every crown is the same shape (instances vary by scale).
    let vertices = pos
        .iter()
        .map(|p| {
            let lump =
                1.0 + 0.09 * (p.x * 7.0).sin() * (p.y * 5.0 + 1.0).cos() + 0.05 * (p.z * 9.0).sin();
            Vertex {
                position: (*p * lump).to_array(),
                normal: p.to_array(),
                ..Default::default()
            }
        })
        .collect();
    MeshData {
        vertices,
        indices: tris.into_iter().flatten().collect(),
    }
}

/// Conifer crown: a cone of radius 1 from z = 0 up to a point at z = 1,
/// with `segments` sides and a flat base.
pub fn cone(segments: u32) -> MeshData {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let slope = 1.0f32.atan2(1.0); // side normal leans up by the cone's angle
    for i in 0..segments {
        let a0 = i as f32 / segments as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / segments as f32 * std::f32::consts::TAU;
        let am = (a0 + a1) / 2.0;
        let n = glam::Vec3::new(am.cos() * slope.cos(), am.sin() * slope.cos(), slope.sin());
        let base = vertices.len() as u32;
        for p in [
            [a0.cos(), a0.sin(), 0.0],
            [a1.cos(), a1.sin(), 0.0],
            [0.0, 0.0, 1.0],
        ] {
            vertices.push(Vertex {
                position: p,
                normal: n.to_array(),
                ..Default::default()
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
        // Base.
        let b = vertices.len() as u32;
        for p in [
            [a1.cos(), a1.sin(), 0.0],
            [a0.cos(), a0.sin(), 0.0],
            [0.0, 0.0, 0.0],
        ] {
            vertices.push(Vertex {
                position: p,
                normal: [0.0, 0.0, -1.0],
                ..Default::default()
            });
        }
        indices.extend_from_slice(&[b, b + 1, b + 2]);
    }
    MeshData { vertices, indices }
}

/// Palm crown: `count` fronds radiating from the top of the trunk, each a
/// thin leaf (top and bottom faces) arching out to radius 1 and drooping to
/// z = −0.45 at its tip.
pub fn fronds(count: u32) -> MeshData {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    // Along a frond: (distance out, height, half width).
    let spine = [
        (0.0, 0.05, 0.02),
        (0.35, 0.12, 0.13),
        (0.7, -0.05, 0.11),
        (1.0, -0.45, 0.01),
    ];
    for k in 0..count {
        let a = k as f32 / count as f32 * std::f32::consts::TAU;
        let (dir, side) = (
            glam::Vec3::new(a.cos(), a.sin(), 0.0),
            glam::Vec3::new(-a.sin(), a.cos(), 0.0),
        );
        for (face, sign) in [(0, 1.0f32), (1, -1.0)] {
            let base = vertices.len() as u32;
            for (i, &(r, z, w)) in spine.iter().enumerate() {
                let c = dir * r + glam::Vec3::Z * z;
                // Normal: across the leaf's slope, up for the top face.
                let next = spine[(i + 1).min(spine.len() - 1)];
                let prev = spine[i.saturating_sub(1)];
                let along =
                    (dir * (next.0 - prev.0) + glam::Vec3::Z * (next.1 - prev.1)).normalize_or(dir);
                let n = (side.cross(along) * -sign).normalize();
                for p in [c - side * w, c + side * w] {
                    vertices.push(Vertex {
                        position: p.to_array(),
                        normal: n.to_array(),
                        ..Default::default()
                    });
                }
            }
            for i in 0..spine.len() as u32 - 1 {
                let (l0, r0, l1, r1) = (
                    base + 2 * i,
                    base + 2 * i + 1,
                    base + 2 * i + 2,
                    base + 2 * i + 3,
                );
                if face == 0 {
                    indices.extend_from_slice(&[l0, r1, r0, l0, l1, r1]);
                } else {
                    indices.extend_from_slice(&[l0, r0, r1, l0, r1, l1]);
                }
            }
        }
    }
    MeshData { vertices, indices }
}

#[cfg(test)]
mod tree_tests {
    use super::*;

    fn outward(m: &MeshData) -> bool {
        m.indices.chunks(3).all(|t| {
            let p = |i: u32| glam::Vec3::from(m.vertices[i as usize].position);
            let n = glam::Vec3::from(m.vertices[t[0] as usize].normal);
            (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).dot(n) > 0.0
        })
    }

    #[test]
    fn tree_meshes_face_outwards() {
        assert_eq!(crown(0).indices.len() / 3, 20);
        assert_eq!(crown(2).indices.len() / 3, 320);
        assert!(outward(&crown(2)));
        assert_eq!(cone(4).indices.len() / 3, 8);
        assert!(outward(&cone(10)));
        assert!(outward(&fronds(8)));
    }
}
