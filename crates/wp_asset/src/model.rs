//! glTF 2.0 models, converted to the engine's frame.

use glam::{Mat3, Mat4, Vec3};
use std::collections::BTreeMap;
use std::path::Path;
use wp_core::Diagnostic;

/// What a part does on a vehicle, from its material's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// `paint…`: drawn in the vehicle's colour (player red, traffic liveries).
    Paint,
    /// `glass…`.
    Glass,
    /// `brake…` or `tail…`: glows brighter while braking.
    BrakeLight,
    /// `head…`: headlamps.
    HeadLight,
    /// Anything else: drawn with the material's own colour.
    Plain,
}

impl Role {
    pub fn from_name(name: &str) -> Role {
        let n = name.to_ascii_lowercase();
        if n.starts_with("paint") {
            Role::Paint
        } else if n.starts_with("glass") {
            Role::Glass
        } else if n.starts_with("brake") || n.starts_with("tail") {
            Role::BrakeLight
        } else if n.starts_with("head") {
            Role::HeadLight
        } else {
            Role::Plain
        }
    }
}

/// A glTF metal-roughness material, reduced to what the renderer draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    pub role: Role,
    /// Linear RGB.
    pub base_color: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    /// Brightest emissive channel × `KHR_materials_emissive_strength`.
    pub emissive: f32,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            name: String::new(),
            role: Role::Plain,
            base_color: [0.8, 0.8, 0.8],
            roughness: 1.0,
            metallic: 1.0,
            emissive: 0.0,
        }
    }
}

/// The triangles of one material, in the engine's frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub material: Material,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// A model in metres: X forward, Y left, Z up (glTF's +Z forward, +Y up
/// and +X left, rotated). The origin should be on the ground; vehicles put
/// it midway between the axles.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub parts: Vec<Part>,
    /// Simpler copies for distance (see [`crate::LOD_RATIOS`]), each with
    /// the same parts in the same order.
    pub lods: Vec<Vec<Part>>,
    pub min: Vec3,
    pub max: Vec3,
    pub triangles: u32,
    /// Textures in the file (not drawn yet: materials use their factors).
    pub textures: u32,
    /// Primitives skipped because they are not triangles (lines, points).
    pub skipped: u32,
}

impl Model {
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }
}

/// glTF (+X left, +Y up, +Z forward) to engine (+X forward, +Y left, +Z up).
fn to_engine() -> Mat4 {
    Mat4::from_cols(
        glam::Vec4::new(0.0, 1.0, 0.0, 0.0), // glTF X → engine Y
        glam::Vec4::new(0.0, 0.0, 1.0, 0.0), // glTF Y → engine Z
        glam::Vec4::new(1.0, 0.0, 0.0, 0.0), // glTF Z → engine X
        glam::Vec4::W,
    )
}

/// Read a model file. `rel` is its project path (for diagnostics); `.gltf`
/// files may refer to `.bin` buffers next to them.
pub fn load_model(root: &Path, rel: &str) -> Result<Model, Diagnostic> {
    let path = root.join(rel);
    let bytes = std::fs::read(&path).map_err(|e| {
        Diagnostic::error("WP-F001", format!("cannot read model: {e}"))
            .in_file(rel)
            .help("check the path; paths are relative to the project root")
    })?;
    parse_model(&bytes, rel, path.parent())
}

/// Parse glTF or GLB bytes. `dir` is where external buffers live.
pub fn parse_model(bytes: &[u8], rel: &str, dir: Option<&Path>) -> Result<Model, Diagnostic> {
    let bad = |msg: String| {
        Diagnostic::error("WP-F001", msg)
            .in_file(rel)
            .help("export glTF 2.0 from Blender (File → Export → glTF 2.0, format .glb)")
    };
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| bad(format!("invalid glTF: {e}")))?;
    let doc = &gltf.document;
    let mut buffers = Vec::new();
    for b in doc.buffers() {
        let data = match b.source() {
            gltf::buffer::Source::Bin => gltf
                .blob
                .clone()
                .ok_or_else(|| bad("the GLB has no binary chunk".into()))?,
            gltf::buffer::Source::Uri(uri) if uri.starts_with("data:") => {
                return Err(Diagnostic::error(
                    "WP-F002",
                    "embedded base64 buffers are not supported",
                )
                .in_file(rel)
                .help("export as .glb (binary), or as .gltf with a separate .bin"));
            }
            gltf::buffer::Source::Uri(uri) => {
                let p = dir.map(|d| d.join(uri)).unwrap_or_else(|| uri.into());
                std::fs::read(&p).map_err(|e| {
                    Diagnostic::error("WP-F002", format!("cannot read buffer `{uri}`: {e}"))
                        .in_file(rel)
                        .help("keep the .bin next to the .gltf, or export a single .glb")
                })?
            }
        };
        buffers.push(data);
    }

    let mut builder = Builder::default();
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    match scene {
        Some(s) => {
            for node in s.nodes() {
                builder.visit(&node, to_engine(), &buffers)?;
            }
        }
        None => {
            for node in doc.nodes() {
                builder.visit(&node, to_engine(), &buffers)?;
            }
        }
    }
    let mut parts: Vec<Part> = builder.parts.into_values().collect();
    parts.retain(|p| !p.indices.is_empty());
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for p in parts.iter().flat_map(|p| &p.positions) {
        min = min.min(Vec3::from(*p));
        max = max.max(Vec3::from(*p));
    }
    if parts.is_empty() {
        min = Vec3::ZERO;
        max = Vec3::ZERO;
    }
    let triangles = parts.iter().map(|p| p.indices.len() as u32 / 3).sum();
    let lods = crate::LOD_RATIOS
        .iter()
        .map(|r| parts.iter().map(|p| crate::simplify(p, *r)).collect())
        .collect();
    Ok(Model {
        lods,
        parts,
        min,
        max,
        triangles,
        textures: doc.textures().count() as u32,
        skipped: builder.skipped,
    })
}

#[derive(Default)]
struct Builder {
    /// Parts by material index (`None`: the default material), in index order.
    parts: BTreeMap<Option<usize>, Part>,
    skipped: u32,
}

impl Builder {
    fn visit(
        &mut self,
        node: &gltf::Node,
        parent: Mat4,
        buffers: &[Vec<u8>],
    ) -> Result<(), Diagnostic> {
        let m = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            for prim in mesh.primitives() {
                if prim.mode() != gltf::mesh::Mode::Triangles {
                    self.skipped += 1;
                    continue;
                }
                self.add(&prim, m, buffers);
            }
        }
        for child in node.children() {
            self.visit(&child, m, buffers)?;
        }
        Ok(())
    }

    fn add(&mut self, prim: &gltf::Primitive, m: Mat4, buffers: &[Vec<u8>]) {
        let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.as_slice()));
        let Some(positions) = reader.read_positions() else {
            self.skipped += 1;
            return;
        };
        let positions: Vec<Vec3> = positions
            .map(|p| m.transform_point3(Vec3::from(p)))
            .collect();
        let normal_m = Mat3::from_mat4(m).inverse().transpose();
        let normals: Option<Vec<Vec3>> = reader.read_normals().map(|n| {
            n.map(|n| (normal_m * Vec3::from(n)).normalize_or_zero())
                .collect()
        });
        let mut indices: Vec<u32> = match reader.read_indices() {
            Some(i) => i.into_u32().collect(),
            None => (0..positions.len() as u32).collect(),
        };
        indices.truncate(indices.len() / 3 * 3);
        indices.retain(|&i| (i as usize) < positions.len());
        indices.truncate(indices.len() / 3 * 3);
        // A mirroring transform turns the winding inside out.
        if Mat3::from_mat4(m).determinant() < 0.0 {
            for t in indices.chunks_mut(3) {
                t.swap(1, 2);
            }
        }
        let normals = normals.unwrap_or_else(|| smooth_normals(&positions, &indices));

        let mat = prim.material();
        let key = mat.index();
        let part = self.parts.entry(key).or_insert_with(|| Part {
            material: material(&mat),
            positions: Vec::new(),
            normals: Vec::new(),
            indices: Vec::new(),
        });
        let base = part.positions.len() as u32;
        part.positions
            .extend(positions.iter().map(|p| p.to_array()));
        part.normals.extend(normals.iter().map(|n| n.to_array()));
        part.indices.extend(indices.iter().map(|i| i + base));
    }
}

fn material(mat: &gltf::Material) -> Material {
    if mat.index().is_none() {
        return Material::default();
    }
    let pbr = mat.pbr_metallic_roughness();
    let c = pbr.base_color_factor();
    let e = mat.emissive_factor();
    let name = mat.name().unwrap_or("").to_string();
    Material {
        role: Role::from_name(&name),
        name,
        base_color: [c[0], c[1], c[2]],
        roughness: pbr.roughness_factor(),
        metallic: pbr.metallic_factor(),
        emissive: e[0].max(e[1]).max(e[2]) * mat.emissive_strength().unwrap_or(1.0),
    }
}

/// Area-weighted vertex normals, for files exported without normals.
fn smooth_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut n = vec![Vec3::ZERO; positions.len()];
    for t in indices.chunks(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
        let face = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        n[a] += face;
        n[b] += face;
        n[c] += face;
    }
    n.into_iter().map(|v| v.normalize_or(Vec3::Z)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-triangle glTF (JSON + external buffer would need files, so a
    /// GLB is assembled in memory).
    fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json = json.as_bytes().to_vec();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut bin = bin.to_vec();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    fn triangle() -> Vec<u8> {
        // glTF frame: a triangle 1 m ahead (+Z), 1 m up (+Y), 1 m left (+X).
        let pts: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]];
        let bin: Vec<u8> = pts.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        let json = r#"{
          "asset": {"version": "2.0"},
          "scene": 0,
          "scenes": [{"nodes": [0]}],
          "nodes": [{"mesh": 0, "translation": [0, 0, 2]}],
          "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "material": 0}]}],
          "materials": [{"name": "paint_body",
            "pbrMetallicRoughness": {"baseColorFactor": [0.5, 0.1, 0.1, 1], "metallicFactor": 0, "roughnessFactor": 0.3}}],
          "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
            "min": [0, 0, 0], "max": [1, 1, 1]}],
          "bufferViews": [{"buffer": 0, "byteLength": 36}],
          "buffers": [{"byteLength": 36}]
        }"#;
        glb(json, &bin)
    }

    #[test]
    fn converts_to_the_engine_frame() {
        let m = parse_model(&triangle(), "t.glb", None).unwrap();
        assert_eq!(m.triangles, 1);
        assert_eq!(m.parts.len(), 1);
        let p = &m.parts[0];
        assert_eq!(p.material.role, Role::Paint);
        assert_eq!(p.material.roughness, 0.3);
        // The node moved it 2 m forward: glTF +Z is engine +X.
        assert_eq!(p.positions[0], [2.0, 0.0, 0.0]);
        assert_eq!(p.positions[1], [3.0, 0.0, 0.0]);
        // glTF +X (left) and +Y (up) → engine +Y and +Z.
        assert_eq!(p.positions[2], [2.0, 1.0, 1.0]);
        assert_eq!(m.min, Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(m.max, Vec3::new(3.0, 1.0, 1.0));
        // Generated normal agrees with the winding.
        let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(p.positions[i]));
        let face = (b - a).cross(c - a).normalize();
        assert!(face.dot(Vec3::from(p.normals[0])) > 0.99);
    }

    #[test]
    fn bad_files_have_codes() {
        let e = parse_model(b"not a model", "x.glb", None).unwrap_err();
        assert_eq!(e.code, "WP-F001");
    }

    #[test]
    fn roles_from_names() {
        assert_eq!(Role::from_name("Paint.001"), Role::Paint);
        assert_eq!(Role::from_name("tail_lamp"), Role::BrakeLight);
        assert_eq!(Role::from_name("headlight"), Role::HeadLight);
        assert_eq!(Role::from_name("rubber"), Role::Plain);
    }
}
