//! Post-processing for stylised looks (`Look::post`): bloom, distance blur,
//! exposure and tone mapping, a colour grade and a vignette (`post.wgsl`).
//! Other looks tone-map in their surface shaders and never come here, so
//! their pictures do not change.

use wp_rhi::{Gpu, wgpu};

/// The scene's linear colour (alpha: distance, m).
pub(crate) const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Levels in the blur pyramid (½, ¼, ⅛, 1/16, 1/32 of the internal size).
const LEVELS: usize = 5;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Params {
    pub a: [f32; 4],
    pub b: [f32; 4],
    pub highlights: [f32; 4],
    pub shade: [f32; 4],
}

pub(crate) struct Post {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    /// The scene (MSAA, and resolved) at the internal size.
    pub(crate) msaa: wgpu::TextureView,
    pub(crate) hdr: wgpu::TextureView,
    /// Halvings, and the way back up (up[k] is the size of down[k]).
    down_levels: Vec<wgpu::TextureView>,
    up_levels: Vec<wgpu::TextureView>,
    size: (u32, u32),
    /// A 1×1 texture for unused bindings.
    blank: wgpu::TextureView,
}

fn texture(gpu: &Gpu, label: &str, w: u32, h: u32, samples: u32) -> wgpu::TextureView {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if samples == 1 {
        usage |= wgpu::TextureUsages::TEXTURE_BINDING;
    }
    gpu.device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

impl Post {
    pub(crate) fn new(gpu: &Gpu, output: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()),
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                tex(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex(3),
                tex(4),
                tex(5),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |fs: &str, format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut post = Post {
            down: pipeline("fs_down", HDR_FORMAT),
            up: pipeline("fs_up", HDR_FORMAT),
            composite: pipeline("fs_composite", output),
            layout,
            sampler,
            params,
            msaa: texture(gpu, "post msaa", 1, 1, crate::MSAA_SAMPLES),
            hdr: texture(gpu, "post hdr", 1, 1, 1),
            down_levels: Vec::new(),
            up_levels: Vec::new(),
            size: (0, 0),
            blank: texture(gpu, "post blank", 1, 1, 1),
        };
        post.resize(gpu, size);
        post
    }

    /// Textures for an internal size of `size`.
    pub(crate) fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.msaa = texture(gpu, "post msaa", size.0, size.1, crate::MSAA_SAMPLES);
        self.hdr = texture(gpu, "post hdr", size.0, size.1, 1);
        let level = |k: usize| ((size.0 >> (k + 1)).max(1), (size.1 >> (k + 1)).max(1));
        self.down_levels = (0..LEVELS)
            .map(|k| texture(gpu, "post down", level(k).0, level(k).1, 1))
            .collect();
        self.up_levels = (0..LEVELS)
            .map(|k| texture(gpu, "post up", level(k).0, level(k).1, 1))
            .collect();
    }

    fn group(
        &self,
        gpu: &Gpu,
        src: &wgpu::TextureView,
        coarse: &wgpu::TextureView,
        bloom: &wgpu::TextureView,
        blur: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        fn view(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(v),
            }
        }
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.layout,
            entries: &[
                view(0, src),
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params.as_entire_binding(),
                },
                view(3, coarse),
                view(4, bloom),
                view(5, blur),
            ],
        })
    }

    fn pass(
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Run the chain on `hdr` (already drawn) into `target`.
    pub(crate) fn encode(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        params: Params,
    ) {
        gpu.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let blank = &self.blank;
        // Down: hdr → ½ → ¼ → ... → 1/32.
        for k in 0..LEVELS {
            let src = if k == 0 {
                &self.hdr
            } else {
                &self.down_levels[k - 1]
            };
            let g = self.group(gpu, src, blank, blank, blank);
            Self::pass(encoder, &self.down_levels[k], &self.down, &g);
        }
        // Up: up[last] = down[last]; up[k] = down[k] + tent(up[k + 1]).
        for k in (0..LEVELS - 1).rev() {
            let coarse = if k + 1 == LEVELS - 1 {
                &self.down_levels[k + 1]
            } else {
                &self.up_levels[k + 1]
            };
            let g = self.group(gpu, &self.down_levels[k], coarse, blank, blank);
            Self::pass(encoder, &self.up_levels[k], &self.up, &g);
        }
        let g = self.group(
            gpu,
            &self.hdr,
            blank,
            &self.up_levels[0],
            &self.down_levels[1],
        );
        Self::pass(encoder, target, &self.composite, &g);
    }
}
