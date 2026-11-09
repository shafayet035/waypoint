//! GPU time per stage of a frame, from timestamp queries (where the GPU has
//! them): marks between passes and, if the GPU allows it, inside the main
//! pass. Results come back a frame or two later and are averaged until
//! [`GpuTimer::report`] is asked for them. Off unless a caller turns it on.

use std::sync::{Arc, Mutex};
use wp_rhi::{Gpu, wgpu};

/// A frame's mark labels being read back, and whether the read finished.
type InFlight = (Vec<&'static str>, Arc<Mutex<Option<bool>>>);

/// Most marks in one frame.
const MAX_MARKS: u32 = 16;

pub(crate) struct GpuTimer {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// Nanoseconds per timestamp tick.
    period: f64,
    /// Can marks go inside a render pass?
    pub(crate) in_passes: bool,
    /// This frame's mark labels, in order.
    labels: Vec<&'static str>,
    /// Labels of the frame being read back, and whether it is ready.
    in_flight: Option<InFlight>,
    /// Summed ms per stage (the label of the mark ending it) and frames.
    sums: Vec<(&'static str, f64)>,
    frames: u32,
}

impl GpuTimer {
    /// A timer, if the GPU can write timestamps between passes.
    pub(crate) fn new(gpu: &Gpu) -> Option<Self> {
        let features = gpu.device.features();
        if !features.contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        ) {
            return None;
        }
        let size = MAX_MARKS as u64 * 8;
        let buffer = |label, usage| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(GpuTimer {
            set: gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("gpu timer"),
                ty: wgpu::QueryType::Timestamp,
                count: MAX_MARKS,
            }),
            resolve: buffer(
                "gpu timer resolve",
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            ),
            readback: buffer(
                "gpu timer readback",
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
            period: gpu.queue.get_timestamp_period() as f64,
            in_passes: features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES),
            labels: Vec::new(),
            in_flight: None,
            sums: Vec::new(),
            frames: 0,
        })
    }

    /// Collect a finished readback, if one is ready; start a new frame.
    pub(crate) fn begin(&mut self, gpu: &Gpu) {
        self.labels.clear();
        let _ = gpu.device.poll(wgpu::PollType::Poll);
        let ready = self
            .in_flight
            .as_ref()
            .is_some_and(|(_, done)| done.lock().is_ok_and(|d| *d == Some(true)));
        if !ready {
            return;
        }
        let (labels, _) = self.in_flight.take().expect("checked");
        {
            let data = self.readback.slice(..).get_mapped_range();
            if let Ok(data) = data {
                let ticks: Vec<u64> = data
                    .chunks(8)
                    .take(labels.len())
                    .map(|c| u64::from_le_bytes(c.try_into().unwrap_or([0; 8])))
                    .collect();
                for k in 1..ticks.len() {
                    let ms = ticks[k].saturating_sub(ticks[k - 1]) as f64 * self.period / 1e6;
                    match self.sums.iter_mut().find(|(l, _)| *l == labels[k]) {
                        Some((_, s)) => *s += ms,
                        None => self.sums.push((labels[k], ms)),
                    }
                }
                self.frames += 1;
            }
        }
        self.readback.unmap();
    }

    /// The GPU has done everything before this point: what came since the
    /// last mark was `label`.
    pub(crate) fn mark(&mut self, encoder: &mut wgpu::CommandEncoder, label: &'static str) {
        if self.in_flight.is_some() || self.labels.len() as u32 >= MAX_MARKS {
            return;
        }
        encoder.write_timestamp(&self.set, self.labels.len() as u32);
        self.labels.push(label);
    }

    /// [`GpuTimer::mark`] inside a render pass (only if the GPU allows it).
    pub(crate) fn mark_in_pass(&mut self, pass: &mut wgpu::RenderPass<'_>, label: &'static str) {
        if !self.in_passes || self.in_flight.is_some() || self.labels.len() as u32 >= MAX_MARKS {
            return;
        }
        pass.write_timestamp(&self.set, self.labels.len() as u32);
        self.labels.push(label);
    }

    /// Copy this frame's marks out (before the encoder is submitted).
    pub(crate) fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if self.in_flight.is_some() || self.labels.len() < 2 {
            return;
        }
        let n = self.labels.len() as u32;
        encoder.resolve_query_set(&self.set, 0..n, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, n as u64 * 8);
    }

    /// After submitting: start reading this frame's marks back.
    pub(crate) fn submitted(&mut self) {
        if self.in_flight.is_some() || self.labels.len() < 2 {
            return;
        }
        let done = Arc::new(Mutex::new(None));
        let flag = done.clone();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                if let Ok(mut d) = flag.lock() {
                    *d = Some(r.is_ok());
                }
            });
        self.in_flight = Some((std::mem::take(&mut self.labels), done));
    }

    /// Average ms per stage since the last report, in frame order.
    pub(crate) fn report(&mut self) -> Vec<(&'static str, f32)> {
        let n = self.frames.max(1) as f64;
        let out = self
            .sums
            .iter()
            .map(|(l, s)| (*l, (s / n) as f32))
            .collect();
        self.sums.clear();
        self.frames = 0;
        out
    }
}
