//! Temporal anti-aliasing and upscaling (see `taa.wgsl`).
//!
//! Windows turn it on ([`crate::Renderer::set_temporal`]); `wp shot` and the
//! screenshot tests draw single frames without it, so their pictures do not
//! depend on what came before. Each frame the scene is drawn with a
//! sub-pixel jitter (a Halton sequence) into the internal-size textures
//! here, together with how far each pixel moved since the last frame
//! (camera motion, plus every vehicle's, trailer's and person's own: see
//! [`crate::FrameView::movers`]); the resolve pass then blends it with the
//! last result at the output size.

use glam::{Mat4, Vec2, Vec3};
use std::collections::BTreeMap;
use wp_rhi::{Gpu, wgpu};

/// Where each pixel moved since the last frame (texture-coordinate units).
pub(crate) const MOTION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
const HISTORY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Moving objects tracked per frame (the uniform's size); slot 0 is "still".
pub(crate) const MAX_MOVERS: usize = 128;
/// A jump further than this between frames (a reset, a respawn, a camera
/// change) is not motion: the history starts again.
const CUT_M: f32 = 8.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    internal: [f32; 4],
    output: [f32; 4],
    jitter: [f32; 4],
}

/// Element `index` (from 1) of the Halton sequence in base `base`, in [0, 1).
fn halton(mut index: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0, 0.0);
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

/// The sub-pixel offset of frame `n`, internal pixels, in (−0.5, 0.5)
/// (image axes: right and down). Eight frames cover a pixel evenly.
pub(crate) fn jitter(n: u32) -> Vec2 {
    if std::env::var("WP_TAA_NOJITTER").is_ok() {
        return Vec2::ZERO;
    }
    let i = n % 8 + 1;
    Vec2::new(halton(i, 2) - 0.5, halton(i, 3) - 0.5)
}

/// `view_proj` with the picture moved by `j` internal pixels of a `w`×`h` target.
pub(crate) fn jittered(view_proj: Mat4, j: Vec2, w: u32, h: u32) -> Mat4 {
    let shift = Mat4::from_translation(Vec3::new(
        2.0 * j.x / w.max(1) as f32,
        -2.0 * j.y / h.max(1) as f32,
        0.0,
    ));
    shift * view_proj
}

/// What the last frame looked like, for motion.
struct Previous {
    view_proj: Mat4,
    eye: Vec3,
    target: Vec3,
    movers: BTreeMap<u32, Mat4>,
}

pub(crate) struct Temporal {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    /// This frame's colour and motion at the internal size (resolved from MSAA).
    pub(crate) scene: wgpu::TextureView,
    pub(crate) motion: wgpu::TextureView,
    pub(crate) motion_msaa: wgpu::TextureView,
    /// Results at the output size, written in turn: frame n writes
    /// `history[n % 2]` and reads the other.
    history: [wgpu::TextureView; 2],
    bind_groups: [wgpu::BindGroup; 2],
    frame: u32,
    valid: bool,
    previous: Option<Previous>,
    internal: (u32, u32),
    output: (u32, u32),
}

/// Motion uniforms for one frame (see `Globals`).
pub(crate) struct FrameMotion {
    /// This frame's camera with its jitter.
    pub jittered: Mat4,
    pub prev_view_proj: Mat4,
    pub eye_delta: Vec3,
    /// Mover id → slot in `motions`.
    pub slots: BTreeMap<u32, u32>,
    pub motions: Vec<Mat4>,
}

impl Temporal {
    pub(crate) fn new(
        gpu: &Gpu,
        format: wgpu::TextureFormat,
        internal: (u32, u32),
        output: (u32, u32),
    ) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("taa"),
            source: wgpu::ShaderSource::Wgsl(include_str!("taa.wgsl").into()),
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
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
            label: Some("taa"),
            entries: &[
                texture(0),
                texture(1),
                texture(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("taa"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("taa"),
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
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: HISTORY_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                ],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("taa"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("taa params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (scene, motion, motion_msaa, history, bind_groups) =
            Self::textures(gpu, &layout, &sampler, &params, format, internal, output);
        Temporal {
            pipeline,
            layout,
            sampler,
            params,
            scene,
            motion,
            motion_msaa,
            history,
            bind_groups,
            frame: 0,
            valid: false,
            previous: None,
            internal,
            output,
        }
    }

    #[allow(clippy::type_complexity)]
    fn textures(
        gpu: &Gpu,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        params: &wgpu::Buffer,
        format: wgpu::TextureFormat,
        (iw, ih): (u32, u32),
        (ow, oh): (u32, u32),
    ) -> (
        wgpu::TextureView,
        wgpu::TextureView,
        wgpu::TextureView,
        [wgpu::TextureView; 2],
        [wgpu::BindGroup; 2],
    ) {
        let make = |label: &str, format, w: u32, h: u32, samples: u32| {
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
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let scene = make("taa scene", format, iw, ih, 1);
        let motion = make("taa motion", MOTION_FORMAT, iw, ih, 1);
        let motion_msaa = make(
            "taa motion (msaa)",
            MOTION_FORMAT,
            iw,
            ih,
            crate::MSAA_SAMPLES,
        );
        let history = [
            make("taa history 0", HISTORY_FORMAT, ow, oh, 1),
            make("taa history 1", HISTORY_FORMAT, ow, oh, 1),
        ];
        let group = |read: &wgpu::TextureView| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("taa"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&scene),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&motion),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(read),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: params.as_entire_binding(),
                    },
                ],
            })
        };
        // Frame writing history[i] reads the other one.
        let bind_groups = [group(&history[1]), group(&history[0])];
        (scene, motion, motion_msaa, history, bind_groups)
    }

    /// New sizes: new textures, and the history starts again.
    pub(crate) fn resize(
        &mut self,
        gpu: &Gpu,
        format: wgpu::TextureFormat,
        internal: (u32, u32),
        output: (u32, u32),
    ) {
        if (internal, output) == (self.internal, self.output) {
            return;
        }
        (
            self.scene,
            self.motion,
            self.motion_msaa,
            self.history,
            self.bind_groups,
        ) = Self::textures(
            gpu,
            &self.layout,
            &self.sampler,
            &self.params,
            format,
            internal,
            output,
        );
        self.internal = internal;
        self.output = output;
        self.valid = false;
    }

    /// Forget the past (a cut: the next frame starts a fresh history).
    pub(crate) fn reset(&mut self) {
        self.valid = false;
        self.previous = None;
    }

    /// This frame's camera matrices and motions. `view_proj` is relative to
    /// `eye`; `movers` are this frame's object transforms.
    pub(crate) fn begin(
        &mut self,
        view_proj: Mat4,
        eye: Vec3,
        target: Vec3,
        movers: &[(u32, Mat4)],
    ) -> FrameMotion {
        let j = jitter(self.frame);
        // A cut (teleport, camera switch): start again rather than smear.
        if let Some(p) = &self.previous {
            let turned = (target - eye)
                .normalize_or_zero()
                .dot((p.target - p.eye).normalize_or_zero());
            if p.eye.distance(eye) > CUT_M || turned < 0.8 {
                self.reset();
            }
        }
        let to_rel = Mat4::from_translation(-eye);
        let from_rel = Mat4::from_translation(eye);
        let mut slots = BTreeMap::new();
        let mut motions = vec![Mat4::IDENTITY];
        let (prev_view_proj, eye_delta) = match &self.previous {
            Some(p) => {
                for (id, now) in movers {
                    if motions.len() >= MAX_MOVERS {
                        break;
                    }
                    let Some(then) = p.movers.get(id) else {
                        continue;
                    };
                    if then.w_axis.distance(now.w_axis) > CUT_M {
                        continue;
                    }
                    // Where a point of this object was, given where it is now.
                    let delta = *then * now.inverse();
                    slots.insert(*id, motions.len() as u32);
                    motions.push(to_rel * delta * from_rel);
                }
                (p.view_proj, eye - p.eye)
            }
            None => (view_proj, Vec3::ZERO),
        };
        self.previous = Some(Previous {
            view_proj,
            eye,
            target,
            movers: movers.iter().copied().collect(),
        });
        FrameMotion {
            jittered: jittered(view_proj, j, self.internal.0, self.internal.1),
            prev_view_proj,
            eye_delta,
            slots,
            motions,
        }
    }

    /// Blend this frame (already drawn into `scene` and `motion`) with the
    /// history into `target` and the next history.
    pub(crate) fn resolve(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        let j = jitter(self.frame);
        let (iw, ih) = (self.internal.0 as f32, self.internal.1 as f32);
        let (ow, oh) = (self.output.0 as f32, self.output.1 as f32);
        let params = Params {
            internal: [iw, ih, 1.0 / iw, 1.0 / ih],
            output: [ow, oh, 1.0 / ow, 1.0 / oh],
            jitter: [
                j.x,
                j.y,
                if self.valid { 1.0 } else { 0.0 },
                std::env::var("WP_TAA_ALPHA")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0),
            ],
        };
        gpu.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let write = (self.frame % 2) as usize;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("taa resolve"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &self.history[write],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_groups[write], &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        self.frame = self.frame.wrapping_add(1);
        self.valid = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_covers_the_pixel_evenly() {
        let pts: Vec<Vec2> = (0..8).map(jitter).collect();
        for p in &pts {
            assert!(p.x.abs() < 0.5 && p.y.abs() < 0.5, "{p:?}");
        }
        // Each half of the pixel gets samples on both axes.
        for axis in [0, 1] {
            let low = pts.iter().filter(|p| p[axis] < 0.0).count();
            assert!((3..=5).contains(&low), "axis {axis}: {low} of 8 below");
        }
        let mean = pts.iter().copied().sum::<Vec2>() / 8.0;
        assert!(mean.length() < 0.1, "{mean:?}");
    }

    #[test]
    fn jitter_moves_the_picture_by_whole_pixels_fractions() {
        let vp = Mat4::IDENTITY;
        let j = Vec2::new(0.25, -0.5);
        let p = jittered(vp, j, 100, 50) * glam::Vec4::new(0.0, 0.0, 0.5, 1.0);
        // 0.25 px right of 100 = 0.005 in NDC; 0.5 px up of 50 = +0.02.
        assert!(
            (p.x - 0.005).abs() < 1e-6 && (p.y - 0.02).abs() < 1e-6,
            "{p:?}"
        );
    }
}
