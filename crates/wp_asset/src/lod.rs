//! Levels of detail: simpler copies of a model for drawing at a distance.
//!
//! Vertex clustering: weld the mesh, snap vertices to a grid, merge each
//! cell into one vertex and drop the triangles that collapse. The cell size
//! grows until the triangle count meets the target. Cheap, robust on any
//! input, and good enough for objects that cover few pixels.

use crate::model::Part;
use glam::Vec3;
use std::collections::BTreeMap;

/// Triangles kept per level, as a share of the full model.
pub const LOD_RATIOS: [f32; 2] = [0.5, 0.25];

/// A copy of `part` with at most about `ratio` of its triangles, flat shaded.
pub fn simplify(part: &Part, ratio: f32) -> Part {
    let target = ((part.indices.len() / 3) as f32 * ratio).ceil() as usize;
    if part.indices.len() < 3 {
        return part.clone();
    }
    // Weld vertices that share a position.
    let mut weld: BTreeMap<[i32; 3], u32> = BTreeMap::new();
    let mut points: Vec<Vec3> = Vec::new();
    let remap: Vec<u32> = part
        .positions
        .iter()
        .map(|p| {
            let key = p.map(|v| (v * 10_000.0).round() as i32);
            *weld.entry(key).or_insert_with(|| {
                points.push(Vec3::from(*p));
                points.len() as u32 - 1
            })
        })
        .collect();
    let tris: Vec<[u32; 3]> = part
        .indices
        .chunks(3)
        .map(|t| {
            [
                remap[t[0] as usize],
                remap[t[1] as usize],
                remap[t[2] as usize],
            ]
        })
        .collect();
    let (min, max) = points.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(a, b), p| (a.min(*p), b.max(*p)),
    );
    let diagonal = (max - min).length().max(1e-3);
    let mut best: Option<Clustered> = None;
    for step in 0..12 {
        let cell = diagonal * 0.01 * 1.5f32.powi(step);
        let (centres, kept) = cluster(&points, &tris, min, cell);
        let done = kept.len() <= target;
        best = Some((centres, kept));
        if done {
            break;
        }
    }
    let (centres, kept) = best.expect("at least one try");
    let mut out = Part {
        material: part.material.clone(),
        positions: Vec::with_capacity(kept.len() * 3),
        normals: Vec::with_capacity(kept.len() * 3),
        uvs: Vec::with_capacity(kept.len() * 3),
        indices: Vec::with_capacity(kept.len() * 3),
    };
    for (orig, t) in kept {
        let [a, b, c] = t.map(|i| centres[i as usize]);
        let n = (b - a).cross(c - a).normalize_or_zero();
        // Texture coordinates from the original triangle's corners.
        for (k, p) in [a, b, c].into_iter().enumerate() {
            let src = part.indices[orig * 3 + k] as usize;
            out.indices.push(out.positions.len() as u32);
            out.positions.push(p.to_array());
            out.normals.push(n.to_array());
            out.uvs
                .push(part.uvs.get(src).copied().unwrap_or([0.0, 0.0]));
        }
    }
    out
}

/// Merged vertex positions, and the triangles kept (with the index of the
/// triangle each came from).
type Clustered = (Vec<Vec3>, Vec<(usize, [u32; 3])>);

/// Merge vertices per grid cell; keep triangles that still have area, with
/// the index of the triangle each came from.
fn cluster(points: &[Vec3], tris: &[[u32; 3]], min: Vec3, cell: f32) -> Clustered {
    let mut cells: BTreeMap<[i32; 3], (u32, Vec3, u32)> = BTreeMap::new();
    let mut of = Vec::with_capacity(points.len());
    for p in points {
        let key = ((*p - min) / cell).floor().to_array().map(|v| v as i32);
        let next = cells.len() as u32;
        let e = cells.entry(key).or_insert((next, Vec3::ZERO, 0));
        e.1 += *p;
        e.2 += 1;
        of.push(e.0);
    }
    let mut centres = vec![Vec3::ZERO; cells.len()];
    for (id, sum, n) in cells.values() {
        centres[*id as usize] = *sum / *n as f32;
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut kept = Vec::new();
    for (n, t) in tris.iter().enumerate() {
        let c = t.map(|i| of[i as usize]);
        if c[0] == c[1] || c[1] == c[2] || c[0] == c[2] {
            continue;
        }
        let mut key = c;
        key.sort_unstable();
        if !seen.insert((key, c)) {
            continue;
        }
        let [a, b, d] = c.map(|i| centres[i as usize]);
        if (b - a).cross(d - a).length_squared() < 1e-12 {
            continue;
        }
        kept.push((n, c));
    }
    (centres, kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Material, Part};

    /// A finely tessellated sphere-ish grid.
    fn dense() -> Part {
        let n = 40;
        let mut part = Part {
            material: Material::default(),
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
        };
        let at = |i: usize, j: usize| {
            let (u, v) = (
                i as f32 / n as f32 * std::f32::consts::TAU,
                j as f32 / n as f32 * std::f32::consts::PI,
            );
            [v.sin() * u.cos(), v.sin() * u.sin(), v.cos()]
        };
        for i in 0..n {
            for j in 0..n {
                for p in [
                    at(i, j),
                    at(i + 1, j),
                    at(i + 1, j + 1),
                    at(i, j),
                    at(i + 1, j + 1),
                    at(i, j + 1),
                ] {
                    part.indices.push(part.positions.len() as u32);
                    part.positions.push(p);
                    part.normals.push(p);
                    part.uvs.push([0.0, 0.0]);
                }
            }
        }
        part
    }

    #[test]
    fn levels_meet_their_targets_and_keep_the_shape() {
        let full = dense();
        let tris = full.indices.len() / 3;
        for ratio in LOD_RATIOS {
            let lod = simplify(&full, ratio);
            let n = lod.indices.len() / 3;
            assert!(
                n <= (tris as f32 * ratio).ceil() as usize,
                "{n} of {tris} for {ratio}"
            );
            assert!(n > tris / 40, "too coarse: {n}");
            // Still about the same size.
            let r = lod
                .positions
                .iter()
                .map(|p| Vec3::from(*p).length())
                .fold(0.0f32, f32::max);
            assert!((0.85..=1.01).contains(&r), "radius {r}");
        }
    }
}
