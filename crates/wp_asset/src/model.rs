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
    /// Base colour texture (sRGB), multiplied by `base_color`.
    pub base_color_texture: Option<std::sync::Arc<Texture>>,
    /// Tangent-space normal map (linear) and its strength.
    pub normal_texture: Option<(std::sync::Arc<Texture>, f32)>,
    /// glTF metallic-roughness map (linear): G scales roughness, B metallic.
    pub metallic_roughness_texture: Option<std::sync::Arc<Texture>>,
    /// Emissive colour map (sRGB), times `emissive`.
    pub emissive_texture: Option<std::sync::Arc<Texture>>,
    /// Ambient occlusion map (linear, R) and its strength.
    pub occlusion_texture: Option<(std::sync::Arc<Texture>, f32)>,
}

/// A decoded image: RGBA8, sRGB, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    /// Pixels; empty when the image is GPU-compressed (`basis`).
    pub rgba: Vec<u8>,
    /// A `.basis` file (shipping builds): transcoded to the GPU's format on upload.
    pub basis: Option<std::sync::Arc<Vec<u8>>>,
}

impl Texture {
    /// An uncompressed image.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Texture {
        Texture {
            width,
            height,
            rgba,
            basis: None,
        }
    }

    /// The pixels, decoding a compressed image if need be.
    pub fn pixels(&self) -> Option<std::borrow::Cow<'_, [u8]>> {
        match &self.basis {
            None => Some(std::borrow::Cow::Borrowed(&self.rgba)),
            Some(b) => crate::basis::transcode(b, crate::basis::GpuFormat::Rgba8)
                .ok()?
                .into_iter()
                .next()
                .map(|l| std::borrow::Cow::Owned(l.data)),
        }
    }
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
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            emissive_texture: None,
            occlusion_texture: None,
        }
    }
}

/// The triangles of one material, in the engine's frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub material: Material,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Texture coordinates (0,0 when the file has none).
    pub uvs: Vec<[f32; 2]>,
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
    /// Textures in the file (base colour textures are drawn).
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

    let mut builder = Builder {
        dir: dir.map(|d| d.to_path_buf()),
        ..Default::default()
    };
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
    /// Decoded images by glTF image index.
    images: BTreeMap<usize, std::sync::Arc<Texture>>,
    /// Folder of the file, for images stored beside it.
    dir: Option<std::path::PathBuf>,
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

        let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
            Some(t) => t.into_f32().collect(),
            None => vec![[0.0, 0.0]; positions.len()],
        };
        let mat = prim.material();
        let key = mat.index();
        if !self.parts.contains_key(&key) {
            let mut m = material(&mat);
            let pbr = mat.pbr_metallic_roughness();
            m.base_color_texture = pbr
                .base_color_texture()
                .and_then(|info| self.image(&info.texture().source(), buffers));
            m.metallic_roughness_texture = pbr
                .metallic_roughness_texture()
                .and_then(|info| self.image(&info.texture().source(), buffers));
            m.normal_texture = mat.normal_texture().and_then(|info| {
                Some((self.image(&info.texture().source(), buffers)?, info.scale()))
            });
            m.emissive_texture = mat
                .emissive_texture()
                .and_then(|info| self.image(&info.texture().source(), buffers));
            m.occlusion_texture = mat.occlusion_texture().and_then(|info| {
                Some((
                    self.image(&info.texture().source(), buffers)?,
                    info.strength(),
                ))
            });

            self.parts.insert(
                key,
                Part {
                    material: m,
                    positions: Vec::new(),
                    normals: Vec::new(),
                    uvs: Vec::new(),
                    indices: Vec::new(),
                },
            );
        }
        let part = self.parts.get_mut(&key).expect("inserted above");
        let base = part.positions.len() as u32;
        part.positions
            .extend(positions.iter().map(|p| p.to_array()));
        part.normals.extend(normals.iter().map(|n| n.to_array()));
        part.uvs.extend(uvs.iter().take(positions.len()));
        part.uvs.resize(part.positions.len(), [0.0, 0.0]);
        part.indices.extend(indices.iter().map(|i| i + base));
    }
}

impl Builder {
    /// Decode a glTF image once (from the GLB's binary chunk or a file
    /// beside the model). Unreadable images are skipped: the material keeps
    /// its colour.
    fn image(&mut self, img: &gltf::Image, buffers: &[Vec<u8>]) -> Option<std::sync::Arc<Texture>> {
        if let Some(t) = self.images.get(&img.index()) {
            return Some(t.clone());
        }
        let bytes: Vec<u8> = match img.source() {
            gltf::image::Source::View { view, .. } => {
                let data = buffers.get(view.buffer().index())?;
                data.get(view.offset()..view.offset() + view.length())?
                    .to_vec()
            }
            gltf::image::Source::Uri { uri, .. } if !uri.starts_with("data:") => {
                std::fs::read(self.dir.as_ref()?.join(uri)).ok()?
            }
            gltf::image::Source::Uri { .. } => return None,
        };
        let t = if crate::basis::is_basis(&bytes) {
            let (width, height, _) = crate::basis::info(&bytes)?;
            std::sync::Arc::new(Texture {
                width,
                height,
                rgba: Vec::new(),
                basis: Some(std::sync::Arc::new(bytes)),
            })
        } else {
            let decoded = image::load_from_memory(&bytes).ok()?.to_rgba8();
            std::sync::Arc::new(Texture::from_rgba(
                decoded.width(),
                decoded.height(),
                decoded.into_raw(),
            ))
        };
        self.images.insert(img.index(), t.clone());
        Some(t)
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
        ..Material::default()
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

    /// A 2×2 checker PNG (red, blue / blue, red).
    fn checker_png() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 2, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[
                255, 0, 0, 255, 0, 0, 255, 255, //
                0, 0, 255, 255, 255, 0, 0, 255,
            ])
            .unwrap();
        }
        out
    }

    /// One textured triangle with texture coordinates, PNG in the binary chunk.
    pub(crate) fn textured_triangle() -> Vec<u8> {
        let pts: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]];
        let uvs: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let mut bin: Vec<u8> = pts.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        bin.extend(uvs.iter().flatten().flat_map(|f| f.to_le_bytes()));
        let png = checker_png();
        let png_at = bin.len();
        bin.extend(&png);
        let json = format!(
            r#"{{
          "asset": {{"version": "2.0"}},
          "scenes": [{{"nodes": [0]}}], "scene": 0,
          "nodes": [{{"mesh": 0}}],
          "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0, "TEXCOORD_0": 1}}, "material": 0}}]}}],
          "materials": [{{"name": "livery", "pbrMetallicRoughness": {{"baseColorTexture": {{"index": 0}}, "metallicRoughnessTexture": {{"index": 0}}}}, "normalTexture": {{"index": 0, "scale": 0.5}}, "occlusionTexture": {{"index": 0, "strength": 0.7}}, "emissiveTexture": {{"index": 0}}}}],
          "textures": [{{"source": 0}}],
          "images": [{{"bufferView": 2, "mimeType": "image/png"}}],
          "accessors": [
            {{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,1]}},
            {{"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2"}}
          ],
          "bufferViews": [
            {{"buffer": 0, "byteOffset": 0, "byteLength": 36}},
            {{"buffer": 0, "byteOffset": 36, "byteLength": 24}},
            {{"buffer": 0, "byteOffset": {png_at}, "byteLength": {}}}
          ],
          "buffers": [{{"byteLength": {}}}]
        }}"#,
            png.len(),
            bin.len()
        );
        glb(&json, &bin)
    }

    #[test]
    fn textures_and_coordinates_load() {
        let m = parse_model(&textured_triangle(), "t.glb", None).unwrap();
        let p = &m.parts[0];
        assert_eq!(p.uvs, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        let t = p.material.base_color_texture.as_ref().expect("texture");
        assert_eq!((t.width, t.height), (2, 2));
        assert_eq!(&t.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(m.textures, 1);
        let mat = &p.material;
        assert_eq!(mat.normal_texture.as_ref().map(|(_, s)| *s), Some(0.5));
        assert_eq!(mat.occlusion_texture.as_ref().map(|(_, s)| *s), Some(0.7));
        assert!(mat.metallic_roughness_texture.is_some() && mat.emissive_texture.is_some());
        // One image, decoded once and shared.
        assert!(std::sync::Arc::ptr_eq(
            t,
            mat.emissive_texture.as_ref().unwrap()
        ));
        // Levels of detail keep the coordinates.
        assert_eq!(m.lods[0][0].uvs.len(), m.lods[0][0].positions.len());
    }
}
