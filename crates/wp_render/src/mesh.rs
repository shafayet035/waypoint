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
