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
    BlobShadow, Camera, CameraMode, DrawItem, Environment, FrameView, MeshId, extract,
};

use bytemuck::{Pod, Zeroable};
use glam::Mat3;
use wp_core::Diagnostic;
use wp_rhi::{Gpu, OffscreenTarget, wgpu};

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
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    normal: [[f32; 4]; 3],
    color: [f32; 4],
    material: [f32; 4],
}

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
    sky: wgpu::RenderPipeline,
    ground: wgpu::RenderPipeline,
    meshes_pipeline: wgpu::RenderPipeline,
    cube: GpuMesh,
    wheel: GpuMesh,
    instances: wgpu::Buffer,
    instance_capacity: usize,
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waypoint"),
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

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<mesh::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
                7 => Float32x4, 8 => Float32x4, 9 => Float32x4,
                10 => Float32x4, 11 => Float32x4
            ],
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
            wgpu::CompareFunction::Less,
            None,
        );
        let meshes_pipeline = pipeline(
            "meshes",
            "vs_mesh",
            "fs_mesh",
            &[Some(vertex_layout), Some(instance_layout)],
            true,
            wgpu::CompareFunction::Less,
            Some(wgpu::Face::Back),
        );

        let instance_capacity = 256;
        let instances = Self::instance_buffer(gpu, instance_capacity);
        let (msaa, depth) = Self::attachments(gpu, format, width, height);
        Self {
            format,
            width,
            height,
            globals,
            bind_group,
            sky,
            ground,
            meshes_pipeline,
            cube: GpuMesh::new(gpu, &mesh::cube(), "cube"),
            wheel: GpuMesh::new(gpu, &mesh::wheel(), "wheel"),
            instances,
            instance_capacity,
            msaa,
            depth,
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
        if (width, height) != (self.width, self.height) && width > 0 && height > 0 {
            self.width = width;
            self.height = height;
            (self.msaa, self.depth) = Self::attachments(gpu, self.format, width, height);
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Draw `frame` into `target` (same size and format as the renderer).
    pub fn render(&mut self, gpu: &Gpu, frame: &FrameView, target: &wgpu::TextureView) {
        let aspect = self.width as f32 / self.height.max(1) as f32;
        let view_proj = frame.camera.view_proj(aspect);
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
        };
        let shadow_count = frame.shadows.len().min(MAX_SHADOWS);
        globals.params[1] = shadow_count as f32;
        for (i, s) in frame.shadows.iter().take(MAX_SHADOWS).enumerate() {
            globals.shadows[i] = [s.center.x, s.center.y, s.half_extents.x, s.half_extents.y];
            globals.shadow_rot[i] = [s.heading.cos(), s.heading.sin(), s.strength, s.softness];
        }
        gpu.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        // Instances grouped by mesh: cubes first, then wheels.
        let raw = |d: &DrawItem| InstanceRaw {
            model: d.transform.to_cols_array_2d(),
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
        let cubes: Vec<InstanceRaw> = frame
            .items
            .iter()
            .filter(|d| d.mesh == MeshId::Cube)
            .map(raw)
            .collect();
        let wheels: Vec<InstanceRaw> = frame
            .items
            .iter()
            .filter(|d| d.mesh == MeshId::Wheel)
            .map(raw)
            .collect();
        let total = cubes.len() + wheels.len();
        if total > self.instance_capacity {
            self.instance_capacity = total.next_power_of_two();
            self.instances = Self::instance_buffer(gpu, self.instance_capacity);
        }
        let mut all = cubes.clone();
        all.extend_from_slice(&wheels);
        if !all.is_empty() {
            gpu.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(&all));
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa,
                    depth_slice: None,
                    resolve_target: Some(target),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
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
            let stride = std::mem::size_of::<InstanceRaw>() as u64;
            let mut first = 0u64;
            for (mesh, count) in [
                (&self.cube, cubes.len() as u64),
                (&self.wheel, wheels.len() as u64),
            ] {
                if count == 0 {
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
                first += count;
            }
        }
        gpu.queue.submit([encoder.finish()]);
    }
}

/// Render one frame of `world` offscreen and return tightly packed RGBA8 pixels.
pub fn render_rgba(
    gpu: &Gpu,
    frame: &FrameView,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, Diagnostic> {
    let target = OffscreenTarget::new(gpu, width, height);
    let mut renderer = Renderer::new(gpu, wp_rhi::OFFSCREEN_FORMAT, width, height);
    renderer.render(gpu, frame, &target.view);
    target.read_rgba(gpu)
}
