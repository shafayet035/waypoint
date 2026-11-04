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

pub mod extract;
pub mod mesh;

pub use extract::{
    BlobShadow, Camera, CameraMode, ChaseSmoothing, DeformedModel, DrawItem, Environment,
    FrameView, MeshId, PointLight, extract, extract_with,
};

use bytemuck::{Pod, Zeroable};
use glam::Mat3;
use wp_core::Diagnostic;
use wp_rhi::{Gpu, OffscreenTarget, wgpu};

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
}

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

/// Road tiles for the current network. Windows build them on a worker
/// thread and keep only tiles near the camera on the GPU (streaming);
/// offscreen renders build and upload everything at once, so shots are
/// complete and repeatable.
struct RoadCache {
    network: usize,
    /// Tiles still being built on a worker thread.
    pending: Option<std::sync::mpsc::Receiver<Vec<wp_road::RoadTile>>>,
    tiles: Vec<wp_road::RoadTile>,
    /// GPU meshes of resident tiles, by index into `tiles`.
    resident: std::collections::BTreeMap<usize, [GpuMesh; 3]>,
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
}

/// Road tiles uploaded per frame while streaming, nearest first, so a
/// teleport never stalls a frame.
const TILE_UPLOADS_PER_FRAME: usize = 4;

/// GPU meshes for a model library: one per part of each model.
struct ModelCache {
    library: usize,
    meshes: Vec<Vec<GpuMesh>>,
}

/// Materials of the road layers, in `wp_road::RoadMeshes` order.
const ROAD_MATERIALS: [([f32; 3], f32); 3] = [
    ([0.045, 0.046, 0.05], 0.85), // asphalt
    ([0.78, 0.78, 0.76], 0.55),   // markings
    ([0.26, 0.255, 0.24], 0.9),   // sidewalks
];

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
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
    sky: wgpu::RenderPipeline,
    ground: wgpu::RenderPipeline,
    meshes_pipeline: wgpu::RenderPipeline,
    cube: GpuMesh,
    wheel: GpuMesh,
    instances: wgpu::Buffer,
    instance_capacity: usize,
    /// Road meshes for the current network (asphalt, markings, sidewalks),
    /// rebuilt only when the network changes.
    roads: Option<RoadCache>,
    road_instances: wgpu::Buffer,
    road_instance_capacity: usize,
    /// Keep road tiles within this distance of the camera (m) and build
    /// them in the background; `None` keeps the whole map resident.
    stream_range: Option<f32>,
    models: Option<ModelCache>,
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
    upscaler: Option<upscale::Upscaler>,
}

impl Renderer {
    /// `format` is the target's colour format; use an sRGB format so the
    /// shader's linear output is encoded correctly.
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat, width: u32, height: u32) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waypoint"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
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
        let bind_group =
            Self::main_bind_group(gpu, &main_layout, &globals, &shadow_view, &shadow_sampler);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waypoint"),
            bind_group_layouts: &[Some(&main_layout)],
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
                        cull: Option<wgpu::Face>| {
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
                    count: MSAA_SAMPLES,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32];
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
        );
        let ground = pipeline(
            "ground",
            "vs_ground",
            "fs_ground",
            &[],
            true,
            wgpu::CompareFunction::Greater,
            None,
        );
        let meshes_pipeline = pipeline(
            "meshes",
            "vs_mesh",
            "fs_mesh",
            &[Some(vertex_layout()), Some(instance_layout())],
            true,
            wgpu::CompareFunction::Greater,
            Some(wgpu::Face::Back),
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
        let (msaa, depth) = Self::attachments(gpu, format, width, height);
        Self {
            format,
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
            sky,
            ground,
            meshes_pipeline,
            cube: GpuMesh::new(gpu, &mesh::cube(), "cube"),
            wheel: GpuMesh::new(gpu, &mesh::wheel(), "wheel"),
            instances,
            instance_capacity,
            roads: None,
            road_instances,
            road_instance_capacity,
            stream_range: None,
            models: None,
            deformed: Default::default(),
            deformed_slots: Vec::new(),
            stats: RenderStats::default(),
            msaa,
            depth,
            out_width: width,
            out_height: height,
            max_height: None,
            upscaler: None,
        }
    }

    /// Cap the internal rendering height (quality tiers): taller windows
    /// are drawn at this height and scaled up. `None` renders at full size.
    pub fn set_max_height(&mut self, gpu: &Gpu, max_height: Option<u32>) {
        self.max_height = max_height;
        self.apply_size(gpu);
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
            self.bind_group = Self::main_bind_group(
                gpu,
                &self.main_layout,
                &self.globals,
                &self.shadow_view,
                &self.shadow_sampler,
            );
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
            ],
        })
    }

    /// Stream road tiles: keep those within `range_m` of the camera on the
    /// GPU and build new maps on a worker thread. `None` (the default) keeps
    /// everything resident and builds synchronously.
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
        if (iw, ih) != (self.width, self.height) {
            self.width = iw;
            self.height = ih;
            (self.msaa, self.depth) = Self::attachments(gpu, self.format, iw, ih);
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
                    sample_count: MSAA_SAMPLES,
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
        let shadow_count = frame.shadows.len().min(MAX_SHADOWS);
        globals.params[1] = shadow_count as f32;
        for (i, s) in frame.shadows.iter().take(MAX_SHADOWS).enumerate() {
            globals.shadows[i] = [s.center.x, s.center.y, s.half_extents.x, s.half_extents.y];
            globals.shadow_rot[i] = [s.heading.cos(), s.heading.sin(), s.strength, s.softness];
        }
        gpu.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        // Road materials for this wetness: wet asphalt is darker and glossy.
        let w = frame.wetness.clamp(0.0, 1.0);
        let road_mats: Vec<InstanceRaw> = ROAD_MATERIALS
            .iter()
            .enumerate()
            .map(|(i, (c, rough))| {
                let (dark, gloss) = match i {
                    0 => (0.45, 0.18),
                    1 => (0.15, 0.3),
                    _ => (0.3, 0.45),
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
                    material: [rough + (gloss - rough) * w, 0.0, 0.0, 0.0],
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
            if self.mesh(d.mesh).is_some() {
                groups.entry(d.mesh).or_default().push(raw(d));
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

        self.update_roads(gpu, frame, eye);
        // Road tiles to draw: resident and in view; three instances each
        // (asphalt, markings, sidewalks), placed relative to the camera.
        let mut road_draws: Vec<usize> = Vec::new();
        let mut road_raw: Vec<InstanceRaw> = Vec::new();
        if let Some(cache) = &self.roads {
            for &i in cache.resident.keys() {
                let t = &cache.tiles[i];
                let offset = glam::Vec3::new(t.centre[0] - eye.x, t.centre[1] - eye.y, -eye.z);
                if !in_view(&view_proj, offset, t) {
                    continue;
                }
                road_draws.push(i);
                let model = glam::Mat4::from_translation(offset).to_cols_array_2d();
                road_raw.extend(road_mats.iter().map(|m| InstanceRaw { model, ..*m }));
            }
        }
        if road_raw.len() > self.road_instance_capacity {
            self.road_instance_capacity = road_raw.len().next_power_of_two();
            self.road_instances = Self::instance_buffer(gpu, self.road_instance_capacity);
        }
        if !road_raw.is_empty() {
            gpu.queue
                .write_buffer(&self.road_instances, 0, bytemuck::cast_slice(&road_raw));
        }
        let mut stats = RenderStats {
            instances: (total + road_raw.len()) as u32,
            road_tiles_resident: self.roads.as_ref().map_or(0, |c| c.resident.len() as u32),
            road_tiles_drawn: road_draws.len() as u32,
            road_tiles_total: self.roads.as_ref().map_or(0, |c| c.tiles.len() as u32),
            ..Default::default()
        };
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
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
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa,
                    depth_slice: None,
                    resolve_target: Some(match &self.upscaler {
                        Some(u) => &u.view,
                        None => target,
                    }),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Discard,
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
            pass.set_pipeline(&self.sky);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.ground);
            pass.draw(0..6, 0..1);
            pass.set_pipeline(&self.meshes_pipeline);
            if let Some(cache) = &self.roads {
                let stride = std::mem::size_of::<InstanceRaw>() as u64;
                for (n, i) in road_draws.iter().enumerate() {
                    for (k, mesh) in cache.resident[i].iter().enumerate() {
                        if mesh.index_count == 0 {
                            continue;
                        }
                        let slot = (n * 3 + k) as u64;
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
            }
            let stride = std::mem::size_of::<InstanceRaw>() as u64;
            let mut first = 0u64;
            for (id, list) in &groups {
                let count = list.len() as u64;
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
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..count as u32);
                stats.draw_calls += 1;
                stats.triangles += mesh.index_count as u64 / 3 * count;
                first += count;
            }
        }
        if let Some(u) = &self.upscaler {
            u.encode(&mut encoder, target);
        }
        gpu.queue.submit([encoder.finish()]);
        self.stats = stats;
    }
}

impl Renderer {
    fn mesh(&self, id: MeshId) -> Option<&GpuMesh> {
        match id {
            MeshId::Cube => Some(&self.cube),
            MeshId::Wheel => Some(&self.wheel),
            MeshId::Model { model, part } => self
                .models
                .as_ref()?
                .meshes
                .get(model as usize)?
                .get(part as usize),
            MeshId::Deformed { slot, part } => self
                .deformed
                .get(self.deformed_slots.get(slot as usize)?)?
                .get(part as usize),
        }
    }

    /// Upload crash-damaged models that changed; forget ones no longer drawn.
    fn update_deformed(&mut self, gpu: &Gpu, frame: &FrameView) {
        self.deformed_slots = frame.deformed.iter().map(|d| d.key).collect();
        for d in &frame.deformed {
            self.deformed.entry(d.key).or_insert_with(|| {
                d.parts
                    .iter()
                    .map(|p| {
                        let data = mesh::MeshData {
                            vertices: p
                                .positions
                                .iter()
                                .zip(&p.normals)
                                .map(|(pos, n)| mesh::Vertex {
                                    position: *pos,
                                    normal: *n,
                                    tint: 0.0,
                                })
                                .collect(),
                            indices: p.indices.clone(),
                        };
                        GpuMesh::new(gpu, &data, "damaged model")
                    })
                    .collect()
            });
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
        let meshes = lib
            .iter()
            .map(|(path, model)| {
                model
                    .parts
                    .iter()
                    .map(|p| {
                        let data = mesh::MeshData {
                            vertices: p
                                .positions
                                .iter()
                                .zip(&p.normals)
                                .map(|(pos, n)| mesh::Vertex {
                                    position: *pos,
                                    normal: *n,
                                    tint: 0.0,
                                })
                                .collect(),
                            indices: p.indices.clone(),
                        };
                        GpuMesh::new(gpu, &data, path)
                    })
                    .collect()
            })
            .collect();
        self.models = Some(ModelCache {
            library: id,
            meshes,
        });
    }

    /// Keep the frame's road tiles on the GPU: rebuild them when the network
    /// changes, then (when streaming) upload tiles coming into range and drop
    /// those far behind.
    fn update_roads(&mut self, gpu: &Gpu, frame: &FrameView, eye: glam::Vec3) {
        let Some(net) = &frame.roads else {
            self.roads = None;
            return;
        };
        let id = std::sync::Arc::as_ptr(net) as usize;
        if self.roads.as_ref().is_none_or(|c| c.network != id) {
            let mut cache = RoadCache {
                network: id,
                pending: None,
                tiles: Vec::new(),
                resident: Default::default(),
            };
            if self.stream_range.is_some() {
                let (tx, rx) = std::sync::mpsc::channel();
                let net = net.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(wp_road::build_tiles(&net, wp_road::TILE_M));
                });
                cache.pending = Some(rx);
            } else {
                cache.tiles = wp_road::build_tiles(net, wp_road::TILE_M);
            }
            self.roads = Some(cache);
        }
        let range = self.stream_range;
        let cache = self.roads.as_mut().expect("set above");
        if let Some(rx) = &cache.pending
            && let Ok(tiles) = rx.try_recv()
        {
            cache.tiles = tiles;
            cache.pending = None;
        }
        let distance = |t: &wp_road::RoadTile| {
            let half = (t.max[0] - t.min[0]).max(t.max[1] - t.min[1]) * 0.5;
            (glam::Vec2::new(t.centre[0], t.centre[1]) - eye.truncate()).length() - half
        };
        let upload = |t: &wp_road::RoadTile| {
            t.layers.clone().map(|l| {
                let data = mesh::MeshData {
                    vertices: l
                        .positions
                        .iter()
                        .zip(&l.normals)
                        .map(|(p, n)| mesh::Vertex {
                            position: *p,
                            normal: *n,
                            tint: 0.0,
                        })
                        .collect(),
                    indices: l.indices,
                };
                GpuMesh::new(gpu, &data, "road tile")
            })
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

/// Whether a road tile at `offset` from the camera can be on screen: false
/// only when all eight corners of its box lie beyond one side of the view.
fn in_view(view_proj: &glam::Mat4, offset: glam::Vec3, t: &wp_road::RoadTile) -> bool {
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
    let target = OffscreenTarget::new(gpu, width, height);
    let mut renderer = Renderer::new(gpu, wp_rhi::OFFSCREEN_FORMAT, width, height);
    renderer.render(gpu, frame, &target.view);
    Ok((target.read_rgba(gpu)?, renderer.stats()))
}
