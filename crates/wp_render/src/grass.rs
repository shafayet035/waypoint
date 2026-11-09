//! Grass (`grass.wgsl`): one instanced draw of tufts placed on the GPU from
//! the scene's grass mask and land heights (uploaded once per scene).

use std::sync::Arc;
use wp_rhi::{Gpu, wgpu};
use wp_world::grass::BuiltGrass;
use wp_world::terrain::BuiltTerrain;

/// Blades per tuft × vertices per blade.
const VERTICES_PER_TUFT: u32 = 15;
/// Most tufts in one frame (the grid round the camera is capped to this).
const MAX_TUFTS: u32 = 160_000;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    a: [f32; 4],
    b: [f32; 4],
    base: [f32; 4],
    tip: [f32; 4],
    land: [f32; 4],
    land_size: [f32; 4],
    mask: [f32; 4],
    mask_size: [f32; 4],
}

pub(crate) struct Grass {
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    /// Plain and linear (post-processing) pipelines.
    pipelines: [wgpu::RenderPipeline; 2],
    /// The scene's grass and land this bind group was made for.
    source: Option<(usize, usize)>,
    group: Option<wgpu::BindGroup>,
    /// What this frame draws: tufts, or none.
    tufts: u32,
    /// The quality tier's share of the reach (Low draws less).
    pub(crate) reach_scale: f32,
}

impl Grass {
    pub(crate) fn new(
        gpu: &Gpu,
        shader: &wgpu::ShaderModule,
        main_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        formats: [wgpu::TextureFormat; 2],
    ) -> Self {
        let device = &gpu.device;
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex(1),
                tex(2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass"),
            bind_group_layouts: &[Some(main_layout), Some(material_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("grass"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_grass"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Greater),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: crate::MSAA_SAMPLES,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_grass"),
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
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grass params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Grass {
            layout,
            params,
            pipelines: [pipeline(formats[0]), pipeline(formats[1])],
            source: None,
            group: None,
            tufts: 0,
            reach_scale: 1.0,
        }
    }

    fn texture(
        gpu: &Gpu,
        label: &str,
        format: wgpu::TextureFormat,
        (w, h): (usize, usize),
        bytes: &[u8],
        texel: usize,
    ) -> wgpu::TextureView {
        let size = wgpu::Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: 1,
        };
        let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            t.as_image_copy(),
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((w * texel) as u32),
                rows_per_image: Some(h as u32),
            },
            size,
        );
        t.create_view(&Default::default())
    }

    /// Prepare this frame's grass (`None`: no grass).
    pub(crate) fn update(
        &mut self,
        gpu: &Gpu,
        grass: Option<&Arc<BuiltGrass>>,
        land: Option<&Arc<BuiltTerrain>>,
    ) {
        let Some(grass) = grass else {
            self.tufts = 0;
            return;
        };
        let key = (
            Arc::as_ptr(grass) as usize,
            land.map_or(0, |l| Arc::as_ptr(l) as usize),
        );
        let def = &grass.def;
        let map = &grass.map;
        if self.source != Some(key) {
            let heights = match land {
                Some(l) => Self::texture(
                    gpu,
                    "grass land",
                    wgpu::TextureFormat::R32Float,
                    (l.field.nx, l.field.ny),
                    bytemuck::cast_slice(&l.field.heights),
                    4,
                ),
                None => Self::texture(
                    gpu,
                    "grass land",
                    wgpu::TextureFormat::R32Float,
                    (1, 1),
                    &[0; 4],
                    4,
                ),
            };
            let mask = Self::texture(
                gpu,
                "grass mask",
                wgpu::TextureFormat::R8Unorm,
                (map.nx, map.ny),
                &map.mask,
                1,
            );
            self.group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("grass"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&heights),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&mask),
                    },
                ],
            }));
            self.source = Some(key);
        }
        let reach = def.draw_m * self.reach_scale;
        let mut spacing = 1.0 / def.density_per_m2.max(0.01).sqrt();
        let mut side = (2.0 * reach / spacing).ceil() as u32 + 1;
        // Too many: wider spacing (fewer, so the far edge stays put).
        while side * side > MAX_TUFTS {
            spacing *= 1.1;
            side = (2.0 * reach / spacing).ceil() as u32 + 1;
        }
        self.tufts = side * side;
        let colour = |c: (u8, u8, u8)| {
            let l = wp_world::srgb([c.0, c.1, c.2]);
            [l[0], l[1], l[2], 1.0]
        };
        let (land_pos, land_size) = match land {
            Some(l) => (
                [l.field.origin.x, l.field.origin.y, l.field.cell_m, 0.0],
                [l.field.nx as f32, l.field.ny as f32, 0.0, 0.0],
            ),
            None => ([0.0; 4], [1.0, 1.0, 0.0, 0.0]),
        };
        let params = Params {
            a: [spacing, side as f32, reach, def.height_m],
            b: [
                def.sway,
                if land.is_some() { 1.0 } else { 0.0 },
                wp_world::terrain::MESH_DROP_M,
                0.0,
            ],
            base: colour(def.colour),
            tip: colour(def.tip_colour),
            land: land_pos,
            land_size,
            mask: [map.origin.x, map.origin.y, map.cell_m, 0.0],
            mask_size: [map.nx as f32, map.ny as f32, 0.0, 0.0],
        };
        gpu.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
    }

    /// Draw this frame's grass; the caller has bound groups 0 and 1.
    /// Returns (draw calls, triangles).
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, linear: bool) -> (u32, u64) {
        let Some(group) = &self.group else {
            return (0, 0);
        };
        if self.tufts == 0 {
            return (0, 0);
        }
        pass.set_pipeline(&self.pipelines[linear as usize]);
        pass.set_bind_group(2, group, &[]);
        pass.draw(0..VERTICES_PER_TUFT, 0..self.tufts);
        (1, self.tufts as u64 * 5)
    }
}
