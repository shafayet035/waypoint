//! The frame pipeline.
//!
//! [`extract`] turns the simulation into a [`FrameView`] (camera, lights, draw
//! items, shadows); [`Renderer::render`] draws it. The renderer only reads the
//! world, so headless simulation never needs a GPU.
//!
//! M0 pipeline (works on every tier): procedural sky, a ground plane with a
//! metric grid and soft blob shadows, and instanced vehicles lit by the sun,
//! a sky/ground ambient term and sky reflections, all with 4× MSAA and ACES
//! tonemapping. Tile-friendly: one render pass, no full-screen post passes.

pub mod bench;
pub mod compare;
pub mod extract;
pub mod governor;
pub mod mesh;

pub use extract::{
    BlobShadow, Camera, CameraMode, ChaseSmoothing, DeformedModel, DrawItem, Environment,
    FrameView, MeshId, PointLight, ViewOptions, Windscreen, WiperSpeed, Wipers, extract,
    extract_view, extract_with,
};

use bytemuck::{Pod, Zeroable};
use glam::Mat3;
use wp_core::Diagnostic;
use wp_rhi::{Gpu, OffscreenTarget, wgpu};

mod gpu_timer;
mod grass;
mod post;
mod upscale;

pub const MSAA_SAMPLES: u32 = 4;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MAX_SHADOWS: usize = 16;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    sky_zenith: [f32; 4],
    sky_horizon: [f32; 4],
    ground_base: [f32; 4],
    ground_line: [f32; 4],
    params: [f32; 4],
    shadows: [[f32; 4]; MAX_SHADOWS],
    shadow_rot: [[f32; 4]; MAX_SHADOWS],
    light_view_proj: [[f32; 4]; 4],
    shadow_params: [f32; 4],
    lights: [[f32; 4]; MAX_LIGHTS * 3],
    /// Art style: toon bands (0 = smooth), saturation, grid (0/1), flat (0/1).
    style: [f32; 4],
    /// Windscreen (see [`Renderer::glass_uniform`]).
    glass: [[f32; 4]; 6],
    /// Road wetness (0–1), rain (0–1), simulated time (s), ground fog.
    weather: [f32; 4],
    /// 1 / target width, 1 / target height, street reflections on (0/1), unused.
    reflect_params: [f32; 4],
    /// Light wrap, fog height scale (m), linear output for post (0/1), ground plane height (m).
    look: [f32; 4],
    /// Tint of sky light in shade.
    shade: [f32; 4],
    /// Where hilly land covers the flat plane (min x, min y, max x, max y);
    /// empty (min > max) without terrain.
    land_rect: [f32; 4],
}

/// Height of the flat ground plane under hilly land, m (just below the
/// land mesh, which stops at height 0 at its edge).
const GROUND_UNDER_LAND_M: f32 = -0.12;
/// Height scale of ground fog, m; stylised haze reaches higher.
const FOG_HEIGHT_M: f32 = 40.0;
const HAZE_HEIGHT_M: f32 = 160.0;

/// Where wet roads get their street reflections: the scene drawn mirrored in
/// the road plane at a share of the internal size, in linear colour.
struct Reflection {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    size: (u32, u32),
    /// The mirrored camera's globals and the bind group using them.
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Colour format of the reflection texture (linear, before tone mapping).
const REFLECTION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The reflection's size, a share of the internal size each way (blurred
/// by rough water anyway).
const REFLECTION_SCALE: f32 = 0.5;

/// Most local lights a frame can use (the uniform's size).
pub const MAX_LIGHTS: usize = 64;

/// The sun's shadow map: one cascade centred on what the camera looks at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSettings {
    /// Texels along each side of the map.
    pub size: u32,
    /// Half the side of the square it covers, m.
    pub extent_m: f32,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        ShadowSettings {
            size: 2048,
            extent_m: 45.0,
        }
    }
}

const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    normal: [[f32; 4]; 3],
    color: [f32; 4],
    material: [f32; 4],
}

/// A square of static geometry (roads or buildings), ready to upload:
/// vertices relative to `centre`, one mesh per material layer.
struct TileData {
    centre: [f32; 2],
    min: [f32; 3],
    max: [f32; 3],
    layers: Vec<mesh::MeshData>,
}

impl TileData {
    fn from_road(t: wp_road::RoadTile) -> Self {
        let layers = t
            .layers
            .into_iter()
            .map(|l| mesh::MeshData {
                vertices: l
                    .positions
                    .iter()
                    .zip(&l.normals)
                    .map(|(p, n)| mesh::Vertex {
                        position: *p,
                        normal: *n,
                        ..Default::default()
                    })
                    .collect(),
                indices: l.indices,
            })
            .collect();
        Self {
            centre: t.centre,
            min: t.min,
            max: t.max,
            layers,
        }
    }

    /// Land tiles: one layer per level of detail.
    fn from_land(t: wp_world::terrain::LandTile) -> Self {
        let srgb = |c: f32| {
            let c = c.clamp(0.0, 1.0);
            let s = if c <= 0.003_130_8 {
                c * 12.92
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round() as u8
        };
        let layers = t
            .levels
            .into_iter()
            .map(|m| mesh::MeshData {
                vertices: (0..m.positions.len())
                    .map(|i| {
                        let c = m.colors[i];
                        mesh::Vertex {
                            position: m.positions[i],
                            normal: m.normals[i],
                            color: [srgb(c[0]), srgb(c[1]), srgb(c[2]), 255],
                            ..Default::default()
                        }
                    })
                    .collect(),
                indices: m.indices,
            })
            .collect();
        Self {
            centre: t.centre,
            min: t.min,
            max: t.max,
            layers,
        }
    }

    /// Water tiles: depth and ripple strength in the texture coordinates,
    /// the deep colour in the vertex colour, the shallow colour and clarity
    /// (0.1 m steps) in the facade slot (`fs_water`).
    fn from_water(t: wp_world::terrain::TerrainTile, w: &wp_world::terrain::WaterDef) -> Self {
        let clarity = (w.clarity_m * 10.0).round().clamp(1.0, 255.0) as u8;
        let vertices = (0..t.positions.len())
            .map(|i| mesh::Vertex {
                position: t.positions[i],
                normal: t.normals[i],
                uv: [t.colors[i][0], w.waves],
                color: [w.deep_colour.0, w.deep_colour.1, w.deep_colour.2, 255],
                facade: [w.colour.0, w.colour.1, w.colour.2, clarity],
                ..Default::default()
            })
            .collect();
        Self {
            centre: t.centre,
            min: t.min,
            max: t.max,
            layers: vec![mesh::MeshData {
                vertices,
                indices: t.indices,
            }],
        }
    }

    fn from_scenery(t: wp_world::scenery::SceneryTile) -> Self {
        let m = t.mesh;
        let vertices = (0..m.positions.len())
            .map(|i| mesh::Vertex {
                position: m.positions[i],
                normal: m.normals[i],
                tint: 0.0,
                uv: m.uvs[i],
                color: m.colors[i],
                facade: m.facades[i],
            })
            .collect();
        Self {
            centre: t.centre,
            min: t.min,
            max: t.max,
            layers: vec![mesh::MeshData {
                vertices,
                indices: m.indices,
            }],
        }
    }
}

/// Static geometry in tiles: road networks and scenery. Windows build the
/// tiles on a worker thread and keep only those near the camera on the GPU
/// (streaming); offscreen renders build and upload everything at once, so
/// shots are complete and repeatable.
struct TileCache {
    /// Identity of the source (the `Arc` it was built from).
    source: usize,
    /// Tiles still being built on a worker thread.
    pending: Option<std::sync::mpsc::Receiver<Vec<TileData>>>,
    tiles: Vec<TileData>,
    /// GPU meshes of resident tiles, by index into `tiles`.
    resident: std::collections::BTreeMap<usize, Vec<GpuMesh>>,
}

impl TileCache {
    /// Keep `slot` in step with `source` (`None`: nothing to draw): rebuild
    /// when it changes, then upload tiles coming into `range` of `eye`
    /// (nearest first) and drop those far behind; `range` `None` keeps all.
    fn sync(
        slot: &mut Option<TileCache>,
        gpu: &Gpu,
        source: Option<usize>,
        build: impl FnOnce() -> Vec<TileData> + Send + 'static,
        range: Option<f32>,
        eye: glam::Vec3,
    ) {
        let Some(id) = source else {
            *slot = None;
            return;
        };
        if slot.as_ref().is_none_or(|c| c.source != id) {
            let mut cache = TileCache {
                source: id,
                pending: None,
                tiles: Vec::new(),
                resident: Default::default(),
            };
            if range.is_some() {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(build());
                });
                cache.pending = Some(rx);
            } else {
                cache.tiles = build();
            }
            *slot = Some(cache);
        }
        let cache = slot.as_mut().expect("set above");
        if let Some(rx) = &cache.pending
            && let Ok(tiles) = rx.try_recv()
        {
            cache.tiles = tiles;
            cache.pending = None;
        }
        let distance = |t: &TileData| {
            let half = (t.max[0] - t.min[0]).max(t.max[1] - t.min[1]) * 0.5;
            (glam::Vec2::new(t.centre[0], t.centre[1]) - eye.truncate()).length() - half
        };
        let upload = |t: &TileData| -> Vec<GpuMesh> {
            t.layers
                .iter()
                .map(|l| GpuMesh::new(gpu, l, "static tile"))
                .collect()
        };
        match range {
            None => {
                for (i, t) in cache.tiles.iter().enumerate() {
                    cache.resident.entry(i).or_insert_with(|| upload(t));
                }
            }
            Some(range) => {
                // Drop tiles well out of range (hysteresis avoids churn at the edge).
                let tiles = &cache.tiles;
                cache
                    .resident
                    .retain(|i, _| distance(&tiles[*i]) < range * 1.25);
                let mut wanted: Vec<(f32, usize)> = tiles
                    .iter()
                    .enumerate()
                    .filter(|(i, t)| !cache.resident.contains_key(i) && distance(t) < range)
                    .map(|(i, t)| (distance(t), i))
                    .collect();
                wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (_, i) in wanted.into_iter().take(TILE_UPLOADS_PER_FRAME) {
                    let meshes = upload(&cache.tiles[i]);
                    cache.resident.insert(i, meshes);
                }
            }
        }
    }
}

/// What the last frame drew: for budgets (draw calls, triangles) and for
/// checking that streaming keeps the GPU's share of the map small.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RenderStats {
    pub draw_calls: u32,
    pub triangles: u64,
    pub instances: u32,
    /// Road tiles on the GPU, drawn this frame, and in the whole map.
    pub road_tiles_resident: u32,
    pub road_tiles_drawn: u32,
    pub road_tiles_total: u32,
    /// Building tiles on the GPU, drawn this frame, and in the whole map.
    #[serde(skip_serializing_if = "is_zero")]
    pub building_tiles_resident: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub building_tiles_drawn: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub building_tiles_total: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// Road tiles uploaded per frame while streaming, nearest first, so a
/// teleport never stalls a frame.
const TILE_UPLOADS_PER_FRAME: usize = 4;

/// GPU meshes for a model library: one per part of each model.
struct ModelCache {
    library: usize,
    /// By model, then level of detail, then part.
    meshes: Vec<Vec<Vec<GpuMesh>>>,
    /// Each model's size (bounding-box diagonal), m, for picking a level.
    sizes: Vec<f32>,
}

/// Materials of the road layers, in `wp_road::RoadMeshes` order (colours
/// come from the game's look; these are the realistic defaults).
const ROAD_MATERIALS: [([f32; 3], f32); 3] = [
    ([0.045, 0.046, 0.05], 0.85), // asphalt
    ([0.78, 0.78, 0.76], 0.55),   // markings
    ([0.26, 0.255, 0.24], 0.9),   // sidewalks
];

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    /// Base colour texture; `None` draws with the plain white one.
    material: Option<std::sync::Arc<wgpu::BindGroup>>,
}

/// Material textures. Each image is uploaded once (with mipmaps, capped to
/// the tier's texture size) and shared; each material gets a bind group
/// with its five maps and their strengths. Missing maps use neutral ones
/// (white, a flat normal), so untextured meshes draw exactly as before.
struct Textures {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The bind group for meshes without any maps.
    white: wgpu::BindGroup,
    white_srgb: std::sync::Arc<wgpu::TextureView>,
    white_linear: std::sync::Arc<wgpu::TextureView>,
    flat_normal: std::sync::Arc<wgpu::TextureView>,
    /// Uploaded images: (image, sRGB?, view).
    views: Vec<(
        std::sync::Arc<wp_asset::Texture>,
        bool,
        std::sync::Arc<wgpu::TextureView>,
    )>,
    /// Material bind groups by their maps and strengths.
    groups: Vec<(MaterialKey, std::sync::Arc<wgpu::BindGroup>)>,
    /// Largest texture side kept (quality tier); bigger images are reduced.
    max_size: u32,
}

/// What makes two materials' bind groups the same.
#[derive(Clone, PartialEq)]
struct MaterialKey {
    maps: [usize; 5],
    params: [u32; 4],
}

/// Per-material shader parameters (group 1, binding 6).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialParams {
    /// x: normal strength (0 = no normal map), y: occlusion strength,
    /// z: 1 when the emissive colour comes from a map, w: unused.
    params: [f32; 4],
}

impl Textures {
    fn new(gpu: &Gpu) -> Textures {
        let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("material"),
                entries: &[
                    texture(0),
                    texture(1),
                    texture(2),
                    texture(3),
                    texture(4),
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let pixel = |rgba: [u8; 4]| wp_asset::Texture::from_rgba(1, 1, rgba.to_vec());
        let white_srgb = std::sync::Arc::new(Self::upload(gpu, &pixel([255; 4]), true, 1));
        let white_linear = std::sync::Arc::new(Self::upload(gpu, &pixel([255; 4]), false, 1));
        let flat_normal =
            std::sync::Arc::new(Self::upload(gpu, &pixel([128, 128, 255, 255]), false, 1));
        let views = [
            &white_srgb,
            &flat_normal,
            &white_linear,
            &white_srgb,
            &white_linear,
        ];
        let white = Self::make_group(gpu, &layout, &sampler, views, [0.0; 4]);
        Textures {
            layout,
            sampler,
            white,
            white_srgb,
            white_linear,
            flat_normal,
            views: Vec::new(),
            groups: Vec::new(),
            max_size: 4096,
        }
    }

    /// Upload an image with a box-filtered mip chain, dropping levels
    /// larger than `max_size`.
    fn upload(gpu: &Gpu, t: &wp_asset::Texture, srgb: bool, max_size: u32) -> wgpu::TextureView {
        if let Some(basis) = &t.basis {
            if let Some(v) = Self::upload_basis(gpu, basis, srgb, max_size) {
                return v;
            }
            // No compressed format on this GPU: decode to RGBA.
            let rgba = t.pixels().map(|p| p.into_owned()).unwrap_or_default();
            let plain = if rgba.len() == (t.width * t.height * 4) as usize {
                wp_asset::Texture::from_rgba(t.width, t.height, rgba)
            } else {
                wp_asset::Texture::from_rgba(1, 1, vec![255; 4])
            };
            return Self::upload(gpu, &plain, srgb, max_size);
        }
        let mut levels = vec![(t.width.max(1), t.height.max(1), t.rgba.clone())];
        while let Some((w, h, px)) = levels.last().cloned()
            && (w > 1 || h > 1)
        {
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut out = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let mut sum = 0u32;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let sx = (x * 2 + dx).min(w - 1);
                            let sy = (y * 2 + dy).min(h - 1);
                            sum += px[((sy * w + sx) * 4 + c) as usize] as u32;
                        }
                        out[((y * nw + x) * 4 + c) as usize] = (sum / 4) as u8;
                    }
                }
            }
            levels.push((nw, nh, out));
        }
        let first = levels
            .iter()
            .position(|(w, h, _)| *w.max(h) <= max_size.max(1))
            .unwrap_or(levels.len() - 1);
        let levels = &levels[first..];
        let (w0, h0, _) = levels[0];
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("material map"),
            size: wgpu::Extent3d {
                width: w0,
                height: h0,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (w, h, px)) in levels.iter().enumerate() {
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(*h),
                },
                wgpu::Extent3d {
                    width: *w,
                    height: *h,
                    depth_or_array_layers: 1,
                },
            );
        }
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Upload a `.basis` image transcoded to a format this GPU samples
    /// directly (ASTC, else ETC2, else BC7). `None` if it has none of them.
    fn upload_basis(
        gpu: &Gpu,
        bytes: &[u8],
        srgb: bool,
        max_size: u32,
    ) -> Option<wgpu::TextureView> {
        use wp_asset::basis::GpuFormat;
        let features = gpu.device.features();
        let channel = if srgb {
            wgpu::AstcChannel::UnormSrgb
        } else {
            wgpu::AstcChannel::Unorm
        };
        let (format, gpu_format) = if features.contains(wgpu::Features::TEXTURE_COMPRESSION_ASTC) {
            (
                GpuFormat::Astc4x4,
                wgpu::TextureFormat::Astc {
                    block: wgpu::AstcBlock::B4x4,
                    channel,
                },
            )
        } else if features.contains(wgpu::Features::TEXTURE_COMPRESSION_ETC2) {
            (
                GpuFormat::Etc2Rgba,
                if srgb {
                    wgpu::TextureFormat::Etc2Rgba8UnormSrgb
                } else {
                    wgpu::TextureFormat::Etc2Rgba8Unorm
                },
            )
        } else if features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
            (
                GpuFormat::Bc7,
                if srgb {
                    wgpu::TextureFormat::Bc7RgbaUnormSrgb
                } else {
                    wgpu::TextureFormat::Bc7RgbaUnorm
                },
            )
        } else {
            return None;
        };
        let levels = wp_asset::basis::transcode(bytes, format).ok()?;
        let first = levels
            .iter()
            .position(|l| l.width.max(l.height) <= max_size.max(1))
            .unwrap_or(levels.len().saturating_sub(1));
        let levels = levels.get(first..)?;
        let (w0, h0) = (levels.first()?.width, levels.first()?.height);
        // Compressed textures need whole blocks at the top level.
        if !w0.is_multiple_of(4) || !h0.is_multiple_of(4) {
            return None;
        }
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("material map (compressed)"),
            size: wgpu::Extent3d {
                width: w0,
                height: h0,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, l) in levels.iter().enumerate() {
            let (bw, bh) = (l.width.div_ceil(4), l.height.div_ceil(4));
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &l.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bw * format.block_bytes()),
                    rows_per_image: Some(bh),
                },
                wgpu::Extent3d {
                    width: bw * 4,
                    height: bh * 4,
                    depth_or_array_layers: 1,
                },
            );
        }
        Some(texture.create_view(&wgpu::TextureViewDescriptor::default()))
    }

    fn make_group(
        gpu: &Gpu,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        views: [&std::sync::Arc<wgpu::TextureView>; 5],
        params: [f32; 4],
    ) -> wgpu::BindGroup {
        let buffer = buffer_init(
            gpu,
            "material params",
            bytemuck::bytes_of(&MaterialParams { params }),
            wgpu::BufferUsages::UNIFORM,
        );
        let mut entries: Vec<wgpu::BindGroupEntry> = views
            .iter()
            .enumerate()
            .map(|(i, v)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::TextureView(v),
            })
            .collect();
        entries.push(wgpu::BindGroupEntry {
            binding: 5,
            resource: wgpu::BindingResource::Sampler(sampler),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 6,
            resource: buffer.as_entire_binding(),
        });
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout,
            entries: &entries,
        })
    }

    /// The view of an image, uploading it the first time.
    fn view(
        &mut self,
        gpu: &Gpu,
        t: &std::sync::Arc<wp_asset::Texture>,
        srgb: bool,
    ) -> std::sync::Arc<wgpu::TextureView> {
        if let Some((_, _, v)) = self
            .views
            .iter()
            .find(|(k, s, _)| std::sync::Arc::ptr_eq(k, t) && *s == srgb)
        {
            return v.clone();
        }
        let v = std::sync::Arc::new(Self::upload(gpu, t, srgb, self.max_size));
        self.views.push((t.clone(), srgb, v.clone()));
        v
    }

    /// A material's bind group, or `None` when it has no maps at all.
    fn group(
        &mut self,
        gpu: &Gpu,
        m: &wp_asset::Material,
    ) -> Option<std::sync::Arc<wgpu::BindGroup>> {
        if m.base_color_texture.is_none()
            && m.normal_texture.is_none()
            && m.metallic_roughness_texture.is_none()
            && m.emissive_texture.is_none()
            && m.occlusion_texture.is_none()
        {
            return None;
        }
        let base = match &m.base_color_texture {
            Some(t) => self.view(gpu, t, true),
            None => self.white_srgb.clone(),
        };
        let normal = match &m.normal_texture {
            Some((t, _)) => self.view(gpu, t, false),
            None => self.flat_normal.clone(),
        };
        let mr = match &m.metallic_roughness_texture {
            Some(t) => self.view(gpu, t, false),
            None => self.white_linear.clone(),
        };
        let emissive = match &m.emissive_texture {
            Some(t) => self.view(gpu, t, true),
            None => self.white_srgb.clone(),
        };
        let occlusion = match &m.occlusion_texture {
            Some((t, _)) => self.view(gpu, t, false),
            None => self.white_linear.clone(),
        };
        let params = [
            m.normal_texture.as_ref().map_or(0.0, |(_, s)| *s),
            m.occlusion_texture.as_ref().map_or(0.0, |(_, s)| *s),
            if m.emissive_texture.is_some() {
                1.0
            } else {
                0.0
            },
            0.0,
        ];
        let views = [&base, &normal, &mr, &emissive, &occlusion];
        let key = MaterialKey {
            maps: views.map(|v| std::sync::Arc::as_ptr(v) as usize),
            params: params.map(f32::to_bits),
        };
        if let Some((_, g)) = self.groups.iter().find(|(k, _)| *k == key) {
            return Some(g.clone());
        }
        let g = std::sync::Arc::new(Self::make_group(
            gpu,
            &self.layout,
            &self.sampler,
            views,
            params,
        ));
        self.groups.push((key, g.clone()));
        Some(g)
    }

    /// A model part as a GPU mesh with its material.
    fn part_mesh(&mut self, gpu: &Gpu, p: &wp_asset::Part, label: &str) -> GpuMesh {
        let data = mesh::MeshData {
            vertices: p
                .positions
                .iter()
                .enumerate()
                .map(|(i, pos)| mesh::Vertex {
                    position: *pos,
                    normal: p.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]),
                    tint: 0.0,
                    uv: p.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                    ..Default::default()
                })
                .collect(),
            indices: p.indices.clone(),
        };
        let mut m = GpuMesh::new(gpu, &data, label);
        m.material = self.group(gpu, &p.material);
        m
    }
}

impl GpuMesh {
    fn new(gpu: &Gpu, data: &mesh::MeshData, label: &str) -> Self {
        let vertices = buffer_init(
            gpu,
            label,
            bytemuck::cast_slice(&data.vertices),
            wgpu::BufferUsages::VERTEX,
        );
        let indices = buffer_init(
            gpu,
            label,
            bytemuck::cast_slice(&data.indices),
            wgpu::BufferUsages::INDEX,
        );
        Self {
            vertices,
            indices,
            index_count: data.indices.len() as u32,
            material: None,
        }
    }
}

fn buffer_init(gpu: &Gpu, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (bytes.len() as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&buffer, 0, bytes);
    buffer
}

pub struct Renderer {
    format: wgpu::TextureFormat,
    /// Samples per pixel (edge smoothing): 4, 2 or 1.
    samples: u32,
    width: u32,
    height: u32,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    main_layout: wgpu::BindGroupLayout,
    /// Globals only: the shadow pass cannot bind the map it draws into.
    shadow_bind_group: wgpu::BindGroup,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_sampler: wgpu::Sampler,
    shadow_view: wgpu::TextureView,
    shadow: Option<ShadowSettings>,
    /// Nearest local lights used per frame (quality tier).
    max_lights: usize,
    /// Above 1 keeps full detail further away; below 1 simplifies sooner.
    lod_bias: f32,
    sky: wgpu::RenderPipeline,
    ground: wgpu::RenderPipeline,
    meshes_pipeline: wgpu::RenderPipeline,
    glass: wgpu::RenderPipeline,
    /// Sky and meshes for the mirrored pass (street reflections).
    mirror_pipelines: [wgpu::RenderPipeline; 2],
    /// Sky, ground, meshes and glass writing linear colour for `post`.
    hdr_pipelines: [wgpu::RenderPipeline; 4],
    /// Lakes: plain and linear (post) targets.
    water_pipelines: [wgpu::RenderPipeline; 2],
    /// Water surface tiles.
    water: Option<TileCache>,
    /// Bloom, distance blur and grading for stylised looks (made on first use).
    post: Option<post::Post>,
    reflection: Option<Reflection>,
    /// Share of the internal size the reflection is drawn at; `None`: off.
    reflection_scale: Option<f32>,
    no_reflection: wgpu::TextureView,
    reflection_sampler: wgpu::Sampler,
    cube: GpuMesh,
    wheel: GpuMesh,
    /// Crown, far crown, cone, far cone, palm fronds.
    trees: [GpuMesh; 5],
    instances: wgpu::Buffer,
    instance_capacity: usize,
    /// Road meshes for the current network (asphalt, markings, sidewalks),
    /// rebuilt only when the network changes.
    roads: Option<TileCache>,
    /// Building meshes for the current scenery, in tiles like the roads.
    buildings: Option<TileCache>,
    /// The land's mesh (hilly scenes), in tiles like the roads.
    terrain: Option<TileCache>,
    /// Swaying grass near the camera.
    grass: grass::Grass,
    /// GPU time per stage, when turned on ([`Renderer::set_gpu_timing`]).
    timer: Option<gpu_timer::GpuTimer>,
    /// Parts of the frame left out, for finding what is slow
    /// ([`Renderer::set_skip`]).
    skip: Vec<String>,
    road_instances: wgpu::Buffer,
    road_instance_capacity: usize,
    /// Keep road tiles within this distance of the camera (m) and build
    /// them in the background; `None` keeps the whole map resident.
    stream_range: Option<f32>,
    models: Option<ModelCache>,
    textures: Textures,
    /// GPU meshes of crash-damaged models by key, and this frame's slots.
    deformed: std::collections::BTreeMap<u64, Vec<GpuMesh>>,
    deformed_slots: Vec<u64>,
    stats: RenderStats,
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// Window (output) size; `width`/`height` are the internal size.
    out_width: u32,
    out_height: u32,
    /// Internal height cap (quality tier), and the upscaler when it applies.
    max_height: Option<u32>,
    /// Share of the capped resolution actually drawn (the governor's first lever).
    render_scale: f32,
    upscaler: Option<upscale::Upscaler>,
}

impl Renderer {
    /// `format` is the target's colour format; use an sRGB format so the
    /// shader's linear output is encoded correctly.
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat, width: u32, height: u32) -> Self {
        Self::with_samples(gpu, format, width, height, MSAA_SAMPLES, false)
    }

    /// [`Renderer::new`] with `samples` per pixel for smoothing edges (4, 2
    /// or 1: none; fewer is much cheaper on phone GPUs that draw in tiles),
    /// and `cheap` shading for budget phones (no detail maps, one shadow
    /// sample, no wet-road streaks; the shader's `CHEAP` constant).
    pub fn with_samples(
        gpu: &Gpu,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        samples: u32,
        cheap: bool,
    ) -> Self {
        let constants: &[(&str, f64)] = &[("CHEAP", if cheap { 1.0 } else { 0.0 })];
        // 1 and 4 work everywhere; 2 only where the GPU offers it for every
        // format drawn (else 4).
        let two = gpu
            .device
            .features()
            .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
            && [format, post::HDR_FORMAT, DEPTH_FORMAT].iter().all(|f| {
                gpu.adapter
                    .get_texture_format_features(*f)
                    .flags
                    .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X2)
            });
        let samples = match samples {
            0 | 1 => 1,
            2 | 3 if two => 2,
            _ => 4,
        };
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waypoint"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("shader.wgsl"),
                    "\n",
                    include_str!("grass.wgsl")
                )
                .into(),
            ),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals (shadow pass)"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let main_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals + shadow map"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow = Some(ShadowSettings::default());
        let shadow_view = Self::shadow_texture(gpu, shadow.map_or(1, |s| s.size));
        let reflection_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("reflection"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Bound where no reflection exists (and while drawing one).
        let no_reflection = Self::colour_texture(gpu, "no reflection", REFLECTION_FORMAT, 1, 1);
        let bind_group = Self::main_bind_group(
            gpu,
            &main_layout,
            &globals,
            &shadow_view,
            &shadow_sampler,
            (&no_reflection, &reflection_sampler),
        );
        let textures = Textures::new(gpu);
        let grass = grass::Grass::new(
            gpu,
            &shader,
            &main_layout,
            &textures.layout,
            [format, post::HDR_FORMAT],
            samples,
            constants,
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waypoint"),
            bind_group_layouts: &[Some(&main_layout), Some(&textures.layout)],
            immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        depth_write: bool,
                        compare: wgpu::CompareFunction,
                        cull: Option<wgpu::Face>,
                        blend: Option<wgpu::BlendState>,
                        (target, samples): (wgpu::TextureFormat, u32)| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32, 12 => Float32x2, 13 => Unorm8x4, 14 => Unorm8x4];
        let vertex_layout = || wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<mesh::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vertex_attrs,
        };
        let instance_attrs = wgpu::vertex_attr_array![
            3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
            7 => Float32x4, 8 => Float32x4, 9 => Float32x4,
            10 => Float32x4, 11 => Float32x4
        ];
        let instance_layout = || wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &instance_attrs,
        };
        let sky = pipeline(
            "sky",
            "vs_sky",
            "fs_sky",
            &[],
            false,
            wgpu::CompareFunction::Always,
            None,
            None,
            (format, samples),
        );
        let ground = pipeline(
            "ground",
            "vs_ground",
            "fs_ground",
            &[],
            true,
            wgpu::CompareFunction::Greater,
            None,
            None,
            (format, samples),
        );
        let meshes_pipeline = pipeline(
            "meshes",
            "vs_mesh",
            "fs_mesh",
            &[Some(vertex_layout()), Some(instance_layout())],
            true,
            wgpu::CompareFunction::Greater,
            Some(wgpu::Face::Back),
            None,
            (format, samples),
        );
        // Rain on the windscreen: blended over the scene, hidden behind the
        // cab (steering wheel, dashboard), writing no depth.
        let glass = pipeline(
            "windscreen",
            "vs_glass",
            "fs_glass",
            &[],
            false,
            wgpu::CompareFunction::Greater,
            None,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            (format, samples),
        );

        // The same scene pipelines writing linear colour (and distance) for
        // the post-processing chain of stylised looks.
        let hdr_target = (post::HDR_FORMAT, samples);
        // Water: blended over what is under it (premultiplied colour), and
        // the alpha channel left alone (it holds distance for post).
        let water_blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        });
        let water_pipelines = [(format, samples), hdr_target].map(|target| {
            pipeline(
                "water",
                "vs_mesh",
                "fs_water",
                &[Some(vertex_layout()), Some(instance_layout())],
                false,
                wgpu::CompareFunction::Greater,
                None,
                water_blend,
                target,
            )
        });
        let hdr_pipelines = [
            pipeline(
                "sky (hdr)",
                "vs_sky",
                "fs_sky",
                &[],
                false,
                wgpu::CompareFunction::Always,
                None,
                None,
                hdr_target,
            ),
            pipeline(
                "ground (hdr)",
                "vs_ground",
                "fs_ground",
                &[],
                true,
                wgpu::CompareFunction::Greater,
                None,
                None,
                hdr_target,
            ),
            pipeline(
                "meshes (hdr)",
                "vs_mesh",
                "fs_mesh",
                &[Some(vertex_layout()), Some(instance_layout())],
                true,
                wgpu::CompareFunction::Greater,
                Some(wgpu::Face::Back),
                None,
                hdr_target,
            ),
            pipeline(
                "windscreen (hdr)",
                "vs_glass",
                "fs_glass",
                &[],
                false,
                wgpu::CompareFunction::Greater,
                None,
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                hdr_target,
            ),
        ];
        // The mirrored pass for street reflections: one sample, linear
        // colour, and front faces culled (a mirror flips the winding).
        let mirror_sky = pipeline(
            "sky (mirrored)",
            "vs_sky",
            "fs_sky_mirror",
            &[],
            false,
            wgpu::CompareFunction::Always,
            None,
            None,
            (REFLECTION_FORMAT, 1),
        );
        let mirror_meshes = pipeline(
            "meshes (mirrored)",
            "vs_mesh",
            "fs_mesh_mirror",
            &[Some(vertex_layout()), Some(instance_layout())],
            true,
            wgpu::CompareFunction::Greater,
            Some(wgpu::Face::Front),
            None,
            (REFLECTION_FORMAT, 1),
        );

        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout()), Some(instance_layout())],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SHADOW_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        let road_instance_capacity = 64;
        let road_instances = Self::instance_buffer(gpu, road_instance_capacity);
        let instance_capacity = 256;
        let instances = Self::instance_buffer(gpu, instance_capacity);
        let (msaa, depth) = Self::attachments(gpu, format, width, height, samples);
        let mut renderer = Self {
            format,
            samples,
            width,
            height,
            globals,
            bind_group,
            main_layout,
            shadow_bind_group,
            shadow_pipeline,
            shadow_sampler,
            shadow_view,
            shadow,
            max_lights: 32,
            lod_bias: 1.0,
            sky,
            ground,
            meshes_pipeline,
            glass,
            mirror_pipelines: [mirror_sky, mirror_meshes],
            hdr_pipelines,
            water_pipelines,
            water: None,
            post: None,
            reflection: None,
            reflection_scale: None,
            no_reflection,
            reflection_sampler,
            cube: GpuMesh::new(gpu, &mesh::cube(), "cube"),
            wheel: GpuMesh::new(gpu, &mesh::wheel(), "wheel"),
            trees: [
                GpuMesh::new(gpu, &mesh::crown(2), "crown"),
                GpuMesh::new(gpu, &mesh::crown(0), "crown (far)"),
                GpuMesh::new(gpu, &mesh::cone(10), "cone"),
                GpuMesh::new(gpu, &mesh::cone(4), "cone (far)"),
                GpuMesh::new(gpu, &mesh::fronds(9), "palm fronds"),
            ],
            instances,
            instance_capacity,
            roads: None,
            road_instances,
            road_instance_capacity,
            buildings: None,
            terrain: None,
            grass,
            timer: None,
            skip: Vec::new(),
            stream_range: None,
            models: None,
            textures,
            deformed: Default::default(),
            deformed_slots: Vec::new(),
            stats: RenderStats::default(),
            msaa,
            depth,
            out_width: width,
            out_height: height,
            max_height: None,
            render_scale: 1.0,
            upscaler: None,
        };
        renderer.set_reflections(gpu, Some(REFLECTION_SCALE));
        renderer
    }

    /// The windscreen for the shader: corner (camera-relative) + width,
    /// across + height, up + rain, the two pivots, (time, cycle, stroke,
    /// arm), (mirrored, park angle, top angle, unused).
    fn glass_uniform(w: &Windscreen, eye: glam::Vec3) -> [[f32; 4]; 6] {
        let (cycle, stroke) = w.wipers.speed.timing().unwrap_or((0.0, 0.0));
        let [a, b] = w.wipers.pivots;
        [
            (w.origin - eye).extend(w.width_m).to_array(),
            w.across.extend(w.height_m).to_array(),
            w.up.extend(w.rain).to_array(),
            [a.x, a.y, b.x, b.y],
            [w.time_s, cycle, stroke, w.wipers.arm_m],
            [
                if w.wipers.mirrored { 1.0 } else { 0.0 },
                extract::windscreen::PARK_RAD,
                extract::windscreen::TOP_RAD,
                extract::windscreen::BLADE_FROM,
            ],
        ]
    }

    /// Cap the internal rendering height (quality tiers): taller windows
    /// are drawn at this height and scaled up. `None` renders at full size.
    pub fn set_max_height(&mut self, gpu: &Gpu, max_height: Option<u32>) {
        self.max_height = max_height;
        self.apply_size(gpu);
    }

    /// Draw at `scale` (0.25–1) of the capped resolution on each side and
    /// scale the result up: the cheapest way to win back frame time.
    pub fn set_render_scale(&mut self, gpu: &Gpu, scale: f32) {
        let scale = scale.clamp(0.25, 1.0);
        if scale != self.render_scale {
            self.render_scale = scale;
            self.apply_size(gpu);
        }
    }

    /// Apply a governor step on top of quality tier `tier`.
    pub fn apply_step(&mut self, gpu: &Gpu, tier: wp_world::QualityTier, step: governor::Step) {
        self.set_render_scale(gpu, step.render_scale);
        let (size, extent_m) = tier.shadow_map();
        let shadows = (step.shadow_scale > 0.0).then(|| ShadowSettings {
            size: ((size as f32 * step.shadow_scale) as u32).max(256),
            extent_m,
        });
        self.set_shadows(gpu, shadows);
        self.set_max_lights(((tier.max_lights() as f32 * step.light_scale) as usize).max(4));
        // Reflections go with the shadow map (the governor's effects lever).
        let reflections = tier.reflections() && step.shadow_scale > 0.0;
        self.set_reflections(gpu, reflections.then_some(REFLECTION_SCALE));
    }

    /// Largest texture side kept on the GPU (quality tier); bigger images
    /// are reduced to their largest mip that fits. Applies to images
    /// uploaded after the call.
    pub fn set_max_texture_size(&mut self, px: u32) {
        self.textures.max_size = px.max(1);
    }

    /// How far models keep their detail (quality tier): 1 is the default.
    pub fn set_lod_bias(&mut self, bias: f32) {
        self.lod_bias = bias.max(0.05);
    }

    /// Use at most `n` local lights per frame, nearest first (quality tier).
    pub fn set_max_lights(&mut self, n: usize) {
        self.max_lights = n.min(MAX_LIGHTS);
    }

    /// Sun shadows from a shadow map of this size and reach (quality tier);
    /// `None` falls back to soft blob shadows under vehicles.
    pub fn set_shadows(&mut self, gpu: &Gpu, shadow: Option<ShadowSettings>) {
        let old = self.shadow.map_or(1, |s| s.size);
        self.shadow = shadow;
        let size = shadow.map_or(1, |s| s.size);
        if size != old {
            self.shadow_view = Self::shadow_texture(gpu, size);
            self.rebind(gpu);
        }
    }

    fn shadow_texture(gpu: &Gpu, size: u32) -> wgpu::TextureView {
        gpu.device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("shadow map"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SHADOW_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    fn main_bind_group(
        gpu: &Gpu,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        shadow: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        (reflection, reflection_sampler): (&wgpu::TextureView, &wgpu::Sampler),
    ) -> wgpu::BindGroup {
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals + shadow map"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(shadow),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(reflection),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(reflection_sampler),
                },
            ],
        })
    }

    /// A sampled colour texture that can also be drawn into.
    fn colour_texture(
        gpu: &Gpu,
        label: &str,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> wgpu::TextureView {
        gpu.device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Rebuild the bind groups that see the shadow map and the reflection
    /// (after either changes).
    fn rebind(&mut self, gpu: &Gpu) {
        let reflection = self
            .reflection
            .as_ref()
            .map_or(&self.no_reflection, |r| &r.color);
        self.bind_group = Self::main_bind_group(
            gpu,
            &self.main_layout,
            &self.globals,
            &self.shadow_view,
            &self.shadow_sampler,
            (reflection, &self.reflection_sampler),
        );
        if let Some(r) = &mut self.reflection {
            r.bind_group = Self::main_bind_group(
                gpu,
                &self.main_layout,
                &r.globals,
                &self.shadow_view,
                &self.shadow_sampler,
                (&self.no_reflection, &self.reflection_sampler),
            );
        }
    }

    /// Street reflections in wet roads, drawn at `scale` of the internal size
    /// each way; `None` turns them off (wet roads mirror only the sky).
    pub fn set_reflections(&mut self, gpu: &Gpu, scale: Option<f32>) {
        self.reflection_scale = scale.map(|s| s.clamp(0.1, 1.0));
        self.sync_reflection(gpu);
    }

    fn sync_reflection(&mut self, gpu: &Gpu) {
        let size = self.reflection_scale.map(|s| {
            (
                ((self.width as f32 * s) as u32).max(1),
                ((self.height as f32 * s) as u32).max(1),
            )
        });
        if size == self.reflection.as_ref().map(|r| r.size) {
            return;
        }
        self.reflection = size.map(|(w, h)| {
            let globals = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("globals (reflection)"),
                size: std::mem::size_of::<Globals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let depth = gpu
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("reflection depth"),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: DEPTH_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default());
            Reflection {
                color: Self::colour_texture(gpu, "reflection", REFLECTION_FORMAT, w, h),
                depth,
                size: (w, h),
                bind_group: Self::main_bind_group(
                    gpu,
                    &self.main_layout,
                    &globals,
                    &self.shadow_view,
                    &self.shadow_sampler,
                    (&self.no_reflection, &self.reflection_sampler),
                ),
                globals,
            }
        });
        self.rebind(gpu);
    }

    /// Stream road tiles: keep those within `range_m` of the camera on the
    /// GPU and build new maps on a worker thread. `None` (the default) keeps
    /// everything resident and builds synchronously.
    /// Render at a quality tier: internal height, sun shadow map, local
    /// lights, model detail and texture size (not streaming, which only
    /// windows use).
    /// Measure GPU time per stage (if the GPU has timestamps); read it with
    /// [`Renderer::gpu_times`].
    /// Leave parts of the frame out, to find what costs the time (profiling
    /// only): any of sky, ground, terrain, roads, buildings, water, grass,
    /// items (vehicles, trees, props), post.
    pub fn set_skip(&mut self, parts: &[&str]) {
        self.skip = parts.iter().map(|p| p.to_string()).collect();
    }

    fn skips(&self, part: &str) -> bool {
        !self.skip.is_empty() && self.skip.iter().any(|s| s == part)
    }

    pub fn set_gpu_timing(&mut self, gpu: &Gpu, on: bool) {
        self.timer = if on {
            gpu_timer::GpuTimer::new(gpu)
        } else {
            None
        };
    }

    /// Average GPU ms per stage since the last call (empty when not timing):
    /// each stage is named by where it ends (shadow, reflection, opaque, grass,
    /// water, scene, post, upscale).
    pub fn gpu_times(&mut self) -> Vec<(&'static str, f32)> {
        self.timer.as_mut().map_or_else(Vec::new, |t| t.report())
    }

    pub fn set_quality(&mut self, gpu: &Gpu, q: wp_world::QualityTier) {
        self.set_max_height(gpu, Some(q.max_height()));
        let (size, extent_m) = q.shadow_map();
        self.set_shadows(gpu, Some(ShadowSettings { size, extent_m }));
        self.set_max_lights(q.max_lights());
        self.set_lod_bias(q.lod_bias());
        self.set_max_texture_size(q.max_texture_px());
        self.set_reflections(gpu, q.reflections().then_some(REFLECTION_SCALE));
        self.grass.reach_scale = q.grass_reach();
    }

    pub fn set_streaming(&mut self, range_m: Option<f32>) {
        self.stream_range = range_m;
    }

    /// What the last frame drew.
    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    /// The size frames are drawn at before any upscaling.
    pub fn internal_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn apply_size(&mut self, gpu: &Gpu) {
        let (w, h) = (self.out_width.max(1), self.out_height.max(1));
        let (iw, ih) = match self.max_height {
            Some(cap) if h > cap => (((w as u64 * cap as u64) / h as u64).max(1) as u32, cap),
            _ => (w, h),
        };
        let (iw, ih) = if self.render_scale < 1.0 {
            (
                ((iw as f32 * self.render_scale) as u32).max(1),
                ((ih as f32 * self.render_scale) as u32).max(1),
            )
        } else {
            (iw, ih)
        };
        if (iw, ih) != (self.width, self.height) {
            self.width = iw;
            self.height = ih;
            (self.msaa, self.depth) = Self::attachments(gpu, self.format, iw, ih, self.samples);
            self.sync_reflection(gpu);
        }
        if (iw, ih) == (w, h) {
            self.upscaler = None;
        } else {
            match &mut self.upscaler {
                Some(u) => u.resize(gpu, self.format, iw, ih),
                None => self.upscaler = Some(upscale::Upscaler::new(gpu, self.format, iw, ih)),
            }
        }
    }

    fn instance_buffer(gpu: &Gpu, capacity: usize) -> wgpu::Buffer {
        gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (capacity * std::mem::size_of::<InstanceRaw>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn attachments(
        gpu: &Gpu,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        samples: u32,
    ) -> (wgpu::TextureView, wgpu::TextureView) {
        let make = |label: &str, format: wgpu::TextureFormat| {
            gpu.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: width.max(1),
                        height: height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        (make("msaa", format), make("depth", DEPTH_FORMAT))
    }

    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) {
        if (width, height) != (self.out_width, self.out_height) && width > 0 && height > 0 {
            self.out_width = width;
            self.out_height = height;
            self.apply_size(gpu);
        }
    }

    /// The output (window) size.
    pub fn size(&self) -> (u32, u32) {
        (self.out_width, self.out_height)
    }

    /// Draw `frame` into `target` (same size and format as the renderer).
    pub fn render(&mut self, gpu: &Gpu, frame: &FrameView, target: &wgpu::TextureView) {
        let aspect = self.width as f32 / self.height.max(1) as f32;
        let view_proj = frame.camera.view_proj_relative(aspect);
        let eye = frame.camera.eye;
        let env = &frame.environment;
        let mut globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            camera_pos: frame.camera.eye.extend(1.0).to_array(),
            sun_dir: env.sun_dir.normalize().extend(env.sun_intensity).to_array(),
            sun_color: env.sun_color.extend(1.0).to_array(),
            sky_zenith: env.sky_zenith.extend(1.0).to_array(),
            sky_horizon: env.sky_horizon.extend(1.0).to_array(),
            ground_base: env.ground_base.extend(1.0).to_array(),
            ground_line: env.ground_line.extend(1.0).to_array(),
            params: [env.fog_density, 0.0, env.exposure, 0.0],
            shadows: [[0.0; 4]; MAX_SHADOWS],
            shadow_rot: [[0.0; 4]; MAX_SHADOWS],
            light_view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            shadow_params: [0.0; 4],
            lights: [[0.0; 4]; MAX_LIGHTS * 3],
            style: [
                frame.look.bands,
                frame.look.saturation,
                if frame.look.grid { 1.0 } else { 0.0 },
                if frame.look.style == wp_world::ArtStyle::Flat {
                    1.0
                } else {
                    0.0
                },
            ],
            glass: frame
                .windscreen
                .map_or([[0.0; 4]; 6], |w| Self::glass_uniform(&w, eye)),
            weather: [
                frame.wetness.clamp(0.0, 1.0),
                frame.rain,
                frame.time_s,
                env.ground_fog,
            ],
            reflect_params: [
                1.0 / self.width.max(1) as f32,
                1.0 / self.height.max(1) as f32,
                0.0,
                0.0,
            ],
            look: [
                frame.look.wrap,
                if frame.look.haze > 0.0 {
                    HAZE_HEIGHT_M
                } else {
                    FOG_HEIGHT_M
                },
                if frame.look.post() { 1.0 } else { 0.0 },
                // The flat ground plane sits under any hilly land.
                if frame.terrain.is_some() {
                    GROUND_UNDER_LAND_M
                } else {
                    0.0
                },
            ],
            shade: glam::Vec3::from(frame.look.shade).extend(1.0).to_array(),
            land_rect: match &frame.terrain {
                Some(t) => {
                    let (lo, hi, c) = (t.field.origin, t.field.max(), t.field.cell_m);
                    [lo.x + c, lo.y + c, hi.x - c, hi.y - c]
                }
                None => [1.0, 1.0, -1.0, -1.0],
            },
        };
        // The nearest lights (ties by order, so frames are repeatable).
        let mut order: Vec<(f32, usize)> = frame
            .lights
            .iter()
            .enumerate()
            .map(|(i, l)| ((l.position - eye).length() - l.range, i))
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let used = order.len().min(self.max_lights);
        for (k, (_, i)) in order.iter().take(used).enumerate() {
            let l = &frame.lights[*i];
            globals.lights[k * 3] = (l.position - eye).extend(l.range).to_array();
            globals.lights[k * 3 + 1] = l.color.extend(l.cone_cos).to_array();
            globals.lights[k * 3 + 2] = l.direction.extend(0.0).to_array();
        }
        globals.params[3] = used as f32;
        if let Some(sh) = self.shadow {
            globals.light_view_proj =
                light_view_proj(env.sun_dir, frame.camera.target, eye, sh).to_cols_array_2d();
            globals.shadow_params = [1.0 / sh.size as f32, 1.0, 0.0, 0.0];
        }
        globals.shadow_params[2] = env.darkness();
        let shadow_count = frame.shadows.len().min(MAX_SHADOWS);
        globals.params[1] = shadow_count as f32;
        for (i, s) in frame.shadows.iter().take(MAX_SHADOWS).enumerate() {
            globals.shadows[i] = [s.center.x, s.center.y, s.half_extents.x, s.half_extents.y];
            globals.shadow_rot[i] = [s.heading.cos(), s.heading.sin(), s.strength, s.softness];
        }
        // Street reflections only matter on wet roads.
        let mirror = self.reflection.is_some() && frame.wetness > 0.01;
        if mirror && let Some(r) = &self.reflection {
            globals.reflect_params[2] = 1.0;
            // The same camera looking at the scene mirrored in the road plane
            // (world z = 0, camera-relative z = −eye height).
            let h = eye.z;
            let flip = glam::Mat4::from_cols(
                glam::Vec4::X,
                glam::Vec4::Y,
                glam::Vec4::NEG_Z,
                glam::Vec4::new(0.0, 0.0, -2.0 * h, 1.0),
            );
            let mirrored_view = view_proj * flip;
            let mut mg = globals;
            mg.view_proj = mirrored_view.to_cols_array_2d();
            mg.inv_view_proj = mirrored_view.inverse().to_cols_array_2d();
            mg.reflect_params[2] = 0.0;
            gpu.queue
                .write_buffer(&r.globals, 0, bytemuck::bytes_of(&mg));
        }
        gpu.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        // Road materials for this wetness: wet asphalt is darker and glossy.
        let w = frame.wetness.clamp(0.0, 1.0);
        let look = &frame.look;
        let road_colours = [look.asphalt, look.markings, look.sidewalk];
        let road_mats: Vec<InstanceRaw> = ROAD_MATERIALS
            .iter()
            .zip(road_colours)
            .enumerate()
            .map(|(i, ((_, rough), c))| {
                // Darkening and roughness when soaked, and how readily
                // puddles form (markings match the asphalt under them).
                let (dark, gloss, pools) = match i {
                    0 => (0.45, 0.18, 1.0),
                    1 => (0.15, 0.3, 1.0),
                    _ => (0.3, 0.45, 0.3),
                };
                let k = 1.0 - dark * w;
                InstanceRaw {
                    model: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    normal: [
                        [1.0, 0.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                    ],
                    color: [c[0] * k, c[1] * k, c[2] * k, 1.0],
                    // w > 0: this surface gathers water (streaks, puddles).
                    material: [rough + (gloss - rough) * w, 0.0, 0.0, pools],
                }
            })
            .collect();

        // Instances grouped by mesh: cubes, wheels, then model parts.
        let to_camera = glam::Mat4::from_translation(-eye);
        let raw = |d: &DrawItem| InstanceRaw {
            model: (to_camera * d.transform).to_cols_array_2d(),
            normal: {
                let n = Mat3::from_mat4(d.transform).inverse().transpose();
                [
                    n.x_axis.extend(0.0).to_array(),
                    n.y_axis.extend(0.0).to_array(),
                    n.z_axis.extend(0.0).to_array(),
                ]
            },
            color: d.color.extend(1.0).to_array(),
            material: [d.roughness, d.metallic, d.emissive, 0.0],
        };
        self.update_models(gpu, frame);
        self.update_deformed(gpu, frame);
        let mut groups: std::collections::BTreeMap<MeshId, Vec<InstanceRaw>> =
            std::collections::BTreeMap::new();
        for d in &frame.items {
            let id = self.level_of_detail(d, eye);
            if self.mesh(id).is_some() {
                groups.entry(id).or_default().push(raw(d));
            }
        }
        let total: usize = groups.values().map(Vec::len).sum();
        if total > self.instance_capacity {
            self.instance_capacity = total.next_power_of_two();
            self.instances = Self::instance_buffer(gpu, self.instance_capacity);
        }
        let all: Vec<InstanceRaw> = groups.values().flatten().copied().collect();
        if !all.is_empty() {
            gpu.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(&all));
        }

        self.update_tiles(gpu, frame, eye);
        self.grass
            .update(gpu, frame.grass.as_ref(), frame.terrain.as_ref());
        // Static tiles to draw (resident and in view), placed relative to the
        // camera: roads with three instances each (asphalt, markings,
        // sidewalks), buildings with one. `static_draws[k]` uses instance k.
        let building_mat = InstanceRaw {
            model: glam::Mat4::IDENTITY.to_cols_array_2d(),
            normal: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            color: [1.0; 4],
            material: [0.85, 0.0, 0.0, 0.0],
        };
        let mut static_draws: Vec<(StaticKind, usize, usize)> = Vec::new();
        let mut water_raw: Vec<InstanceRaw> = Vec::new();
        let mut static_raw: Vec<InstanceRaw> = Vec::new();
        let tile_model = |t: &TileData| {
            let offset = glam::Vec3::new(t.centre[0] - eye.x, t.centre[1] - eye.y, -eye.z);
            (
                offset,
                glam::Mat4::from_translation(offset).to_cols_array_2d(),
            )
        };
        let mut road_tiles_drawn = 0;
        if let Some(cache) = &self.roads {
            for &i in cache.resident.keys() {
                let (offset, model) = tile_model(&cache.tiles[i]);
                if !in_view(&view_proj, offset, &cache.tiles[i]) {
                    continue;
                }
                road_tiles_drawn += 1;
                for (k, m) in road_mats.iter().enumerate() {
                    static_draws.push((StaticKind::Road, i, k));
                    static_raw.push(InstanceRaw { model, ..*m });
                }
            }
        }
        if let Some(cache) = &self.terrain {
            for &i in cache.resident.keys() {
                let (offset, model) = tile_model(&cache.tiles[i]);
                if !in_view(&view_proj, offset, &cache.tiles[i]) {
                    continue;
                }
                static_draws.push((StaticKind::Terrain, i, land_level(&cache.tiles[i], eye)));
                static_raw.push(InstanceRaw {
                    model,
                    material: [0.92, 0.0, 0.0, 0.0],
                    ..building_mat
                });
            }
        }
        let mut building_tiles_drawn = 0;
        if let Some(cache) = &self.buildings {
            for &i in cache.resident.keys() {
                let (offset, model) = tile_model(&cache.tiles[i]);
                if !in_view(&view_proj, offset, &cache.tiles[i]) {
                    continue;
                }
                building_tiles_drawn += 1;
                static_draws.push((StaticKind::Building, i, 0));
                static_raw.push(InstanceRaw {
                    model,
                    ..building_mat
                });
            }
        }
        // Lakes: drawn after everything opaque (instances after the static
        // ones, not in `static_draws`, so no other pass draws them).
        let mut water_draws: Vec<(usize, usize)> = Vec::new();
        if let Some(cache) = &self.water {
            for &i in cache.resident.keys() {
                let (offset, model) = tile_model(&cache.tiles[i]);
                if !in_view(&view_proj, offset, &cache.tiles[i]) {
                    continue;
                }
                water_draws.push((i, usize::MAX));
                water_raw.push(InstanceRaw {
                    model,
                    ..building_mat
                });
            }
        }
        // Buildings and hills near the shadow map's square cast shadows, even off screen.
        let main_static = static_draws.len();
        if self.shadow.is_some()
            && let Some(cache) = &self.terrain
        {
            let light = glam::Mat4::from_cols_array_2d(&globals.light_view_proj);
            for &i in cache.resident.keys() {
                let (offset, model) = tile_model(&cache.tiles[i]);
                if in_view(&light, offset, &cache.tiles[i]) {
                    static_draws.push((StaticKind::Terrain, i, land_level(&cache.tiles[i], eye)));
                    static_raw.push(InstanceRaw {
                        model,
                        ..building_mat
                    });
                }
            }
        }
        if self.shadow.is_some()
            && let Some(cache) = &self.buildings
        {
            let light = glam::Mat4::from_cols_array_2d(&globals.light_view_proj);
            for &i in cache.resident.keys() {
                let (offset, model) = tile_model(&cache.tiles[i]);
                if in_view(&light, offset, &cache.tiles[i]) {
                    static_draws.push((StaticKind::Building, i, 0));
                    static_raw.push(InstanceRaw {
                        model,
                        ..building_mat
                    });
                }
            }
        }
        let water_first = static_raw.len();
        for (k, w) in water_draws.iter_mut().enumerate() {
            w.1 = water_first + k;
        }
        static_raw.extend(water_raw);
        if static_raw.len() > self.road_instance_capacity {
            self.road_instance_capacity = static_raw.len().next_power_of_two();
            self.road_instances = Self::instance_buffer(gpu, self.road_instance_capacity);
        }
        if !static_raw.is_empty() {
            gpu.queue
                .write_buffer(&self.road_instances, 0, bytemuck::cast_slice(&static_raw));
        }
        let mut stats = RenderStats {
            instances: (total + main_static) as u32,
            road_tiles_resident: self.roads.as_ref().map_or(0, |c| c.resident.len() as u32),
            road_tiles_drawn,
            road_tiles_total: self.roads.as_ref().map_or(0, |c| c.tiles.len() as u32),
            building_tiles_resident: self
                .buildings
                .as_ref()
                .map_or(0, |c| c.resident.len() as u32),
            building_tiles_drawn,
            building_tiles_total: self.buildings.as_ref().map_or(0, |c| c.tiles.len() as u32),
            ..Default::default()
        };
        // Stylised looks draw into the post chain's linear buffer.
        let post_on = frame.look.post() && !self.skips("post");
        if post_on {
            let size = (self.width, self.height);
            match &mut self.post {
                Some(p) => p.resize(gpu, size),
                None => self.post = Some(post::Post::new(gpu, self.format, size, self.samples)),
            }
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let mut timer = self.timer.take();
        if let Some(t) = timer.as_mut() {
            t.begin(gpu);
            t.mark(&mut encoder, "start");
        }
        if self.shadow.is_some() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.shadow_bind_group, &[]);
            pass.set_pipeline(&self.shadow_pipeline);
            let stride = std::mem::size_of::<InstanceRaw>() as u64;
            let mut first = 0u64;
            for (id, list) in &groups {
                let count = list.len() as u64;
                if let Some(mesh) = self.mesh(*id)
                    && mesh.index_count > 0
                    && count > 0
                {
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_vertex_buffer(
                        1,
                        self.instances
                            .slice(first * stride..(first + count) * stride),
                    );
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..count as u32);
                    stats.draw_calls += 1;
                }
                first += count;
            }
            for (slot, &(kind, tile, layer)) in static_draws.iter().enumerate().skip(main_static) {
                if let Some(mesh) = self.static_mesh(kind, tile, layer) {
                    let slot = slot as u64;
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_vertex_buffer(
                        1,
                        self.road_instances
                            .slice(slot * stride..(slot + 1) * stride),
                    );
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    stats.draw_calls += 1;
                }
            }
        }
        if let Some(t) = timer.as_mut()
            && self.shadow.is_some()
        {
            t.mark(&mut encoder, "shadow");
        }
        if mirror && let Some(r) = &self.reflection {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("reflection"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &r.color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &r.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &r.bind_group, &[]);
            pass.set_bind_group(1, &self.textures.white, &[]);
            let [sky, meshes] = &self.mirror_pipelines;
            pass.set_pipeline(sky);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(meshes);
            let stride = std::mem::size_of::<InstanceRaw>() as u64;
            // Buildings (the road surfaces are the mirror itself).
            for (slot, &(kind, tile, layer)) in static_draws.iter().enumerate().take(main_static) {
                if kind != StaticKind::Building {
                    continue;
                }
                let Some(mesh) = self.static_mesh(kind, tile, layer) else {
                    continue;
                };
                let slot = slot as u64;
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_vertex_buffer(
                    1,
                    self.road_instances
                        .slice(slot * stride..(slot + 1) * stride),
                );
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                stats.draw_calls += 1;
            }
            let mut first = 0u64;
            for (id, list) in &groups {
                let count = list.len() as u64;
                if let Some(mesh) = self.mesh(*id)
                    && count > 0
                    && mesh.index_count > 0
                {
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_vertex_buffer(
                        1,
                        self.instances
                            .slice(first * stride..(first + count) * stride),
                    );
                    pass.set_bind_group(
                        1,
                        mesh.material.as_deref().unwrap_or(&self.textures.white),
                        &[],
                    );
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..count as u32);
                    stats.draw_calls += 1;
                }
                first += count;
            }
        }
        if let Some(t) = timer.as_mut()
            && mirror
        {
            t.mark(&mut encoder, "reflection");
        }
        {
            let resolved = match (&self.post, post_on, &self.upscaler) {
                (Some(p), true, _) => &p.hdr,
                (_, _, Some(u)) => &u.view,
                _ => target,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    // With several samples, draw into the multisampled
                    // buffer and resolve; with one, straight into the target.
                    view: match (self.samples > 1, &self.post, post_on) {
                        (true, Some(p), true) => &p.msaa,
                        (true, _, _) => &self.msaa,
                        (false, _, _) => resolved,
                    },
                    depth_slice: None,
                    resolve_target: (self.samples > 1).then_some(resolved),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: if self.samples > 1 {
                            wgpu::StoreOp::Discard
                        } else {
                            wgpu::StoreOp::Store
                        },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0), // reversed-Z: 0 is the far plane
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_bind_group(1, &self.textures.white, &[]);
            let [hdr_sky, hdr_ground, hdr_meshes, hdr_glass] = &self.hdr_pipelines;
            let pick = |plain, linear| if post_on { linear } else { plain };
            if !self.skips("sky") {
                pass.set_pipeline(pick(&self.sky, hdr_sky));
                pass.draw(0..3, 0..1);
            }
            if !self.skips("ground") {
                pass.set_pipeline(pick(&self.ground, hdr_ground));
                // Round hilly land the plane is four strips framing it.
                pass.draw(0..if frame.terrain.is_some() { 24 } else { 6 }, 0..1);
            }
            pass.set_pipeline(pick(&self.meshes_pipeline, hdr_meshes));
            let stride = std::mem::size_of::<InstanceRaw>() as u64;
            for (slot, &(kind, tile, layer)) in static_draws.iter().enumerate().take(main_static) {
                let part = match kind {
                    StaticKind::Road => "roads",
                    StaticKind::Building => "buildings",
                    StaticKind::Terrain => "terrain",
                    StaticKind::Water => "water",
                };
                if self.skips(part) {
                    continue;
                }
                let Some(mesh) = self.static_mesh(kind, tile, layer) else {
                    continue;
                };
                let slot = slot as u64;
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_vertex_buffer(
                    1,
                    self.road_instances
                        .slice(slot * stride..(slot + 1) * stride),
                );
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                stats.draw_calls += 1;
                stats.triangles += mesh.index_count as u64 / 3;
            }
            let stride = std::mem::size_of::<InstanceRaw>() as u64;
            let mut first = 0u64;
            for (id, list) in &groups {
                let count = list.len() as u64;
                if self.skips("items") {
                    first += count;
                    continue;
                }
                let Some(mesh) = self.mesh(*id) else {
                    continue;
                };
                if count == 0 || mesh.index_count == 0 {
                    first += count;
                    continue;
                }
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_vertex_buffer(
                    1,
                    self.instances
                        .slice(first * stride..(first + count) * stride),
                );
                pass.set_bind_group(
                    1,
                    mesh.material.as_deref().unwrap_or(&self.textures.white),
                    &[],
                );
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..count as u32);
                stats.draw_calls += 1;
                stats.triangles += mesh.index_count as u64 / 3 * count;
                first += count;
            }
            if let Some(t) = timer.as_mut() {
                t.mark_in_pass(&mut pass, "opaque");
            }
            let (calls, tris) = if self.skips("grass") {
                (0, 0)
            } else {
                self.grass.draw(&mut pass, post_on)
            };
            if let Some(t) = timer.as_mut() {
                t.mark_in_pass(&mut pass, "grass");
            }
            stats.draw_calls += calls;
            stats.triangles += tris;
            if !water_draws.is_empty() && !self.skips("water") {
                pass.set_pipeline(&self.water_pipelines[post_on as usize]);
                pass.set_bind_group(1, &self.textures.white, &[]);
                for &(tile, slot) in &water_draws {
                    let Some(mesh) = self.static_mesh(StaticKind::Water, tile, 0) else {
                        continue;
                    };
                    let slot = slot as u64;
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_vertex_buffer(
                        1,
                        self.road_instances
                            .slice(slot * stride..(slot + 1) * stride),
                    );
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    stats.draw_calls += 1;
                    stats.triangles += mesh.index_count as u64 / 3;
                }
            }
            if let Some(t) = timer.as_mut()
                && !water_draws.is_empty()
            {
                t.mark_in_pass(&mut pass, "water");
            }
            if frame.windscreen.is_some() {
                pass.set_pipeline(pick(&self.glass, hdr_glass));
                pass.set_bind_group(1, &self.textures.white, &[]);
                pass.draw(0..6, 0..1);
                stats.draw_calls += 1;
                stats.triangles += 2;
            }
        }
        if let Some(t) = timer.as_mut() {
            t.mark(&mut encoder, "scene");
        }
        if post_on && let Some(p) = &self.post {
            let look = &frame.look;
            let params = post::Params {
                a: [env.exposure, look.saturation, look.bloom, look.depth_blur],
                b: [
                    look.vignette,
                    (frame.camera.target - frame.camera.eye).length(),
                    1.0 / self.width.max(1) as f32,
                    1.0 / self.height.max(1) as f32,
                ],
                highlights: glam::Vec3::from(look.highlights).extend(1.0).to_array(),
                shade: glam::Vec3::from(look.shade).extend(1.0).to_array(),
            };
            let out = self.upscaler.as_ref().map_or(target, |u| &u.view);
            p.encode(gpu, &mut encoder, out, params);
            if let Some(t) = timer.as_mut() {
                t.mark(&mut encoder, "post");
            }
        }
        if let Some(u) = &self.upscaler {
            u.encode(&mut encoder, target);
            if let Some(t) = timer.as_mut() {
                t.mark(&mut encoder, "upscale");
            }
        }
        if let Some(t) = timer.as_mut() {
            t.resolve(&mut encoder);
        }
        gpu.queue.submit([encoder.finish()]);
        if let Some(t) = timer.as_mut() {
            t.submitted();
        }
        self.timer = timer;
        self.stats = stats;
    }
}

/// Which tile cache a static draw comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaticKind {
    Road,
    Building,
    Terrain,
    Water,
}

impl Renderer {
    /// Layer `layer` of tile `tile`, if resident and not empty.
    fn static_mesh(&self, kind: StaticKind, tile: usize, layer: usize) -> Option<&GpuMesh> {
        let cache = match kind {
            StaticKind::Road => self.roads.as_ref()?,
            StaticKind::Building => self.buildings.as_ref()?,
            StaticKind::Terrain => self.terrain.as_ref()?,
            StaticKind::Water => self.water.as_ref()?,
        };
        cache
            .resident
            .get(&tile)?
            .get(layer)
            .filter(|m| m.index_count > 0)
    }

    fn mesh(&self, id: MeshId) -> Option<&GpuMesh> {
        match id {
            MeshId::Cube => Some(&self.cube),
            MeshId::Wheel => Some(&self.wheel),
            MeshId::Crown => Some(&self.trees[0]),
            MeshId::CrownFar => Some(&self.trees[1]),
            MeshId::Cone => Some(&self.trees[2]),
            MeshId::ConeFar => Some(&self.trees[3]),
            MeshId::Fronds => Some(&self.trees[4]),
            MeshId::Model { model, part, lod } => self
                .models
                .as_ref()?
                .meshes
                .get(model as usize)?
                .get(lod as usize)?
                .get(part as usize),
            MeshId::Deformed { slot, part } => self
                .deformed
                .get(self.deformed_slots.get(slot as usize)?)?
                .get(part as usize),
        }
    }

    /// Draw distant models simpler: the level follows how big the model looks
    /// (its size over its distance), scaled by the tier's bias.
    fn level_of_detail(&self, d: &DrawItem, eye: glam::Vec3) -> MeshId {
        if let MeshId::Crown | MeshId::Cone = d.mesh {
            let size = d.transform.x_axis.truncate().length() * 2.0;
            let distance = (d.transform.w_axis.truncate() - eye).length().max(0.1);
            if size / distance * self.lod_bias < 0.05 {
                return if d.mesh == MeshId::Crown {
                    MeshId::CrownFar
                } else {
                    MeshId::ConeFar
                };
            }
            return d.mesh;
        }
        let MeshId::Model { model, part, .. } = d.mesh else {
            return d.mesh;
        };
        let Some(cache) = &self.models else {
            return d.mesh;
        };
        let size = cache.sizes.get(model as usize).copied().unwrap_or(1.0);
        let levels = cache.meshes.get(model as usize).map_or(1, Vec::len);
        let distance = (d.transform.w_axis.truncate() - eye).length().max(0.1);
        let apparent = size / distance * self.lod_bias;
        let lod = if apparent > 0.1 {
            0
        } else if apparent > 0.04 {
            1
        } else {
            2
        }
        .min(levels.saturating_sub(1)) as u8;
        MeshId::Model { model, part, lod }
    }

    /// Upload crash-damaged models that changed; forget ones no longer drawn.
    fn update_deformed(&mut self, gpu: &Gpu, frame: &FrameView) {
        self.deformed_slots = frame.deformed.iter().map(|d| d.key).collect();
        for d in &frame.deformed {
            if !self.deformed.contains_key(&d.key) {
                let meshes = d
                    .parts
                    .iter()
                    .map(|p| self.textures.part_mesh(gpu, p, "damaged model"))
                    .collect();
                self.deformed.insert(d.key, meshes);
            }
        }
        let keep: std::collections::BTreeSet<u64> = self.deformed_slots.iter().copied().collect();
        self.deformed.retain(|k, _| keep.contains(k));
    }

    /// Upload the frame's model library if it changed.
    fn update_models(&mut self, gpu: &Gpu, frame: &FrameView) {
        let Some(lib) = &frame.models else {
            self.models = None;
            return;
        };
        let id = std::sync::Arc::as_ptr(lib) as usize;
        if self.models.as_ref().is_some_and(|c| c.library == id) {
            return;
        }
        let textures = &mut self.textures;
        let meshes = lib
            .iter()
            .map(|(path, model)| {
                std::iter::once(&model.parts)
                    .chain(&model.lods)
                    .map(|parts| {
                        parts
                            .iter()
                            .map(|p| textures.part_mesh(gpu, p, path))
                            .collect()
                    })
                    .collect()
            })
            .collect();
        let sizes = lib.iter().map(|(_, m)| m.size().length()).collect();
        self.models = Some(ModelCache {
            library: id,
            meshes,
            sizes,
        });
    }

    /// Keep the frame's road, building and land tiles on the GPU.
    fn update_tiles(&mut self, gpu: &Gpu, frame: &FrameView, eye: glam::Vec3) {
        let range = self.stream_range;
        let net = frame.roads.clone();
        let land = frame.terrain.clone();
        // Roads lie on the land, so they are rebuilt when either changes.
        let land_id = land
            .as_ref()
            .map_or(0, |t| std::sync::Arc::as_ptr(t) as usize);
        let field = land.as_ref().map(|t| t.field.clone());
        TileCache::sync(
            &mut self.roads,
            gpu,
            net.as_ref()
                .map(|n| (std::sync::Arc::as_ptr(n) as usize) ^ land_id.rotate_left(17)),
            move || {
                net.map(|n| {
                    let mut meshes = wp_road::build_meshes(&n);
                    if let Some(f) = &field {
                        meshes.drape(|p| f.height(p), |p| f.gradient(p));
                    }
                    wp_road::split_tiles(&meshes, wp_road::TILE_M)
                        .into_iter()
                        .map(TileData::from_road)
                        .collect()
                })
                .unwrap_or_default()
            },
            range,
            eye,
        );
        let water = land.clone().filter(|t| !t.water.is_empty());
        TileCache::sync(
            &mut self.water,
            gpu,
            water.as_ref().map(|_| land_id),
            move || {
                water
                    .map(|t| {
                        t.water
                            .iter()
                            .enumerate()
                            .flat_map(|(k, w)| {
                                wp_world::terrain::build_water_tiles(
                                    &t.field,
                                    &t.water_owner,
                                    k as u8,
                                    w.level_m,
                                )
                                .into_iter()
                                .map(|tile| TileData::from_water(tile, w))
                                .collect::<Vec<_>>()
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            },
            range,
            eye,
        );
        // The land is coloured from the look's ground colour: rebuild if it changes.
        let ground = glam::Vec3::from(frame.look.ground);
        let colour_id = ground
            .to_array()
            .iter()
            .fold(0usize, |h, c| h.rotate_left(11) ^ c.to_bits() as usize);
        TileCache::sync(
            &mut self.terrain,
            gpu,
            land.as_ref().map(|_| land_id ^ colour_id),
            move || {
                land.map(|t| {
                    wp_world::terrain::build_tiles(&t.def, &t.field, ground)
                        .into_iter()
                        .map(TileData::from_land)
                        .collect()
                })
                .unwrap_or_default()
            },
            range,
            eye,
        );
        let scenery = frame.scenery.clone();
        TileCache::sync(
            &mut self.buildings,
            gpu,
            scenery.as_ref().map(|s| std::sync::Arc::as_ptr(s) as usize),
            move || {
                scenery
                    .map(|s| {
                        wp_world::scenery::build_tiles(&s, wp_road::TILE_M)
                            .into_iter()
                            .map(TileData::from_scenery)
                            .collect()
                    })
                    .unwrap_or_default()
            },
            range,
            eye,
        );
    }
}

/// The sun's view and orthographic projection around `focus` (world), for
/// camera-relative positions. The square is snapped to whole texels in light
/// space so shadow edges do not shimmer as the camera moves.
fn light_view_proj(
    sun_dir: glam::Vec3,
    focus: glam::Vec3,
    eye: glam::Vec3,
    sh: ShadowSettings,
) -> glam::Mat4 {
    let sun = sun_dir.normalize();
    let up = if sun.z.abs() > 0.99 {
        glam::Vec3::Y
    } else {
        glam::Vec3::Z
    };
    let view = glam::camera::rh::view::look_at_mat4(sun * 300.0, glam::Vec3::ZERO, up);
    let mut f = view.transform_point3(focus);
    let texel = 2.0 * sh.extent_m / sh.size as f32;
    f.x = (f.x / texel).round() * texel;
    f.y = (f.y / texel).round() * texel;
    let e = sh.extent_m;
    // Light looks down its −Z: depths run from in front of the focus to behind it.
    let proj = glam::camera::rh::proj::directx::orthographic(
        f.x - e,
        f.x + e,
        f.y - e,
        f.y + e,
        -f.z - 200.0,
        -f.z + 200.0,
    );
    proj * view * glam::Mat4::from_translation(eye)
}

/// Which level of detail of a land tile to draw from `eye`: full detail
/// within 120 m of it, every other grid point within 350 m, then every fourth.
fn land_level(t: &TileData, eye: glam::Vec3) -> usize {
    let lo = glam::Vec2::new(t.centre[0] + t.min[0], t.centre[1] + t.min[1]);
    let hi = glam::Vec2::new(t.centre[0] + t.max[0], t.centre[1] + t.max[1]);
    let d = eye.truncate().clamp(lo, hi).distance(eye.truncate());
    let level = if d < 120.0 {
        0
    } else if d < 350.0 {
        1
    } else {
        2
    };
    level.min(t.layers.len().saturating_sub(1))
}

/// Whether a tile at `offset` from the camera can be on screen: false
/// only when all eight corners of its box lie beyond one side of the view.
fn in_view(view_proj: &glam::Mat4, offset: glam::Vec3, t: &TileData) -> bool {
    let mut out = [true; 5];
    for k in 0..8 {
        let p = offset
            + glam::Vec3::new(
                if k & 1 == 0 { t.min[0] } else { t.max[0] },
                if k & 2 == 0 { t.min[1] } else { t.max[1] },
                if k & 4 == 0 { t.min[2] } else { t.max[2] },
            );
        let c = *view_proj * p.extend(1.0);
        let inside = [c.x >= -c.w, c.x <= c.w, c.y >= -c.w, c.y <= c.w, c.w > 0.0];
        for (o, i) in out.iter_mut().zip(inside) {
            *o &= !i;
        }
    }
    !out.iter().any(|&o| o)
}

/// Render one frame of `world` offscreen and return tightly packed RGBA8 pixels.
pub fn render_rgba(
    gpu: &Gpu,
    frame: &FrameView,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, Diagnostic> {
    render_rgba_with_stats(gpu, frame, width, height).map(|(rgba, _)| rgba)
}

/// [`render_rgba`] plus what the frame drew (draw calls, triangles, tiles).
pub fn render_rgba_with_stats(
    gpu: &Gpu,
    frame: &FrameView,
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, RenderStats), Diagnostic> {
    render_rgba_at(gpu, frame, width, height, None)
}

/// [`render_rgba_with_stats`] at a quality tier (`None`: the offscreen
/// defaults `wp shot` uses).
pub fn render_rgba_at(
    gpu: &Gpu,
    frame: &FrameView,
    width: u32,
    height: u32,
    quality: Option<wp_world::QualityTier>,
) -> Result<(Vec<u8>, RenderStats), Diagnostic> {
    let target = OffscreenTarget::new(gpu, width, height);
    let mut renderer = Renderer::new(gpu, wp_rhi::OFFSCREEN_FORMAT, width, height);
    if let Some(q) = quality {
        renderer.set_quality(gpu, q);
    }
    renderer.render(gpu, frame, &target.view);
    Ok((target.read_rgba(gpu)?, renderer.stats()))
}
