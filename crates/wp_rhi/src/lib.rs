//! Render hardware interface: a thin layer over `wgpu`.
//!
//! wgpu runs on Vulkan (Android, Linux, Windows), Metal (iOS, macOS) and DX12.
//! This crate owns device setup, offscreen targets and frame capture, so the
//! renderer never deals with platform details. A native Vulkan/Metal path can
//! be added behind the same types later if wgpu blocks a mobile feature.

pub use wgpu;

use std::path::Path;
use wp_core::Diagnostic;

/// Colour format for offscreen targets and screenshots (linear shader output,
/// sRGB storage).
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// An initialised GPU: instance, adapter, device and queue.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// What the engine is running on, for logs and `--json` output.
#[derive(Clone, Debug, serde::Serialize)]
pub struct GpuInfo {
    pub name: String,
    pub backend: String,
    pub device_type: String,
}

impl Gpu {
    /// A GPU with no window, for screenshots and tests.
    pub fn headless() -> Result<Gpu, Diagnostic> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        Self::from_instance(instance, None)
    }

    /// A GPU that can present to `surface`, created from `instance`.
    pub fn for_surface(
        instance: wgpu::Instance,
        surface: &wgpu::Surface<'_>,
    ) -> Result<Gpu, Diagnostic> {
        Self::from_instance(instance, Some(surface))
    }

    fn from_instance(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Gpu, Diagnostic> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: surface,
            ..Default::default()
        }))
        .map_err(|e| {
            Diagnostic::error("WP-R001", format!("no usable GPU adapter: {e}")).help(
                "on Linux CI install a software Vulkan driver (mesa lavapipe); set WGPU_BACKEND to pick a backend",
            )
        })?;
        // Ask only for limits every target supports, phones included.
        let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("waypoint"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| {
            Diagnostic::error(
                "WP-R002",
                format!("the GPU refused to create a device: {e}"),
            )
        })?;
        Ok(Gpu {
            instance,
            adapter,
            device,
            queue,
        })
    }

    pub fn info(&self) -> GpuInfo {
        let i = self.adapter.get_info();
        GpuInfo {
            name: i.name,
            backend: format!("{:?}", i.backend),
            device_type: format!("{:?}", i.device_type),
        }
    }
}

/// A colour texture to render into without a window.
pub struct OffscreenTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl OffscreenTarget {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width,
            height,
        }
    }

    /// Copy the texture back to the CPU as tightly packed RGBA8 rows.
    pub fn read_rgba(&self, gpu: &Gpu) -> Result<Vec<u8>, Diagnostic> {
        let unpadded = self.width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| Diagnostic::error("WP-R003", format!("GPU readback failed: {e}")))?;
        rx.recv()
            .map_err(|_| Diagnostic::error("WP-R003", "GPU readback was cancelled"))?
            .map_err(|e| Diagnostic::error("WP-R003", format!("GPU readback failed: {e}")))?;
        let data = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|e| Diagnostic::error("WP-R003", format!("GPU readback failed: {e}")))?;
        let mut out = Vec::with_capacity((unpadded * self.height) as usize);
        for row in data.chunks(padded as usize) {
            out.extend_from_slice(&row[..unpadded as usize]);
        }
        drop(data);
        buffer.unmap();
        Ok(out)
    }
}

/// Encode RGBA8 pixels as a PNG file.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, Diagnostic> {
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut w = enc.write_header().map_err(png_error)?;
        w.write_image_data(rgba).map_err(png_error)?;
    }
    Ok(bytes)
}

pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), Diagnostic> {
    let bytes = encode_png(width, height, rgba)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| write_error(path, e))?;
    }
    std::fs::write(path, bytes).map_err(|e| write_error(path, e))
}

fn png_error(e: png::EncodingError) -> Diagnostic {
    Diagnostic::error("WP-R010", format!("PNG encoding failed: {e}"))
}

fn write_error(path: &Path, e: std::io::Error) -> Diagnostic {
    Diagnostic::error("WP-R010", format!("cannot write {}: {e}", path.display()))
}
