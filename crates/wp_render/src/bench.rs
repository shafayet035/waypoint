//! The renderer benchmark: a fixed, demanding scene drawn offscreen, to
//! measure what a device can hold. The player runs it on first launch to pick
//! a quality tier (`QualityTier::for_device`); `wp bench` prints it.
//!
//! The scene is the engine's own, not a game's, so numbers compare across
//! games and devices: a rainy night in a 3 × 3 grid of four-lane streets
//! lined with generated buildings and trees, lit street lamps, and 40 cars
//! with headlights, seen from a chase camera moving down the middle avenue.
//! It is drawn at Mid settings and 1280 × 720.

use crate::extract::{self, Camera, Environment, FrameView, PointLight};
use crate::{DrawItem, MeshId, Renderer};
use glam::{Mat4, Vec2, Vec3};
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;
use wp_core::Diagnostic;
use wp_rhi::{Gpu, GpuInfo, OFFSCREEN_FORMAT, OffscreenTarget};
use wp_road::{DriveSide, NodeDef, RoadDef, RoadNetwork, RoadNetworkDef, RoadShape};
use wp_world::QualityTier;
use wp_world::scenery::{Scenery, SceneryDef, StreetDef, TreeRowDef};

pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
/// Frames timed after the warm-up.
pub const FRAMES: u32 = 40;
const WARM_UP: u32 = 4;
const SPACING_M: f32 = 180.0;

/// What the benchmark measured.
#[derive(Clone, Debug, Serialize)]
pub struct BenchReport {
    /// Median frame time, ms (each frame waited for, so CPU and GPU time add).
    pub frame_ms: f32,
    /// 90th-percentile frame time, ms.
    pub p90_ms: f32,
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub draw_calls: u32,
    pub triangles: u64,
    pub gpu: GpuInfo,
}

impl BenchReport {
    /// A CPU rasteriser rather than a GPU (llvmpipe, SwiftShader, WARP).
    pub fn software(&self) -> bool {
        self.gpu.device_type == "Cpu"
    }
}

/// Draw the benchmark on `gpu` and time it.
pub fn run(gpu: &Gpu) -> Result<BenchReport, Diagnostic> {
    let mut frame = scene();
    let mut renderer = Renderer::new(gpu, OFFSCREEN_FORMAT, WIDTH, HEIGHT);
    renderer.set_quality(gpu, QualityTier::Mid);
    let target = OffscreenTarget::new(gpu, WIDTH, HEIGHT);
    let wait = |gpu: &Gpu| {
        gpu.device
            .poll(wp_rhi::wgpu::PollType::wait_indefinitely())
            .map_err(|e| Diagnostic::error("WP-R003", format!("GPU wait failed: {e}")))
    };
    let mut times = Vec::with_capacity(FRAMES as usize);
    for i in 0..WARM_UP + FRAMES {
        frame.camera = camera(i as f32 / (WARM_UP + FRAMES) as f32);
        let start = Instant::now();
        renderer.render(gpu, &frame, &target.view);
        wait(gpu)?;
        if i >= WARM_UP {
            times.push(start.elapsed().as_secs_f32() * 1000.0);
        }
    }
    times.sort_by(f32::total_cmp);
    let stats = renderer.stats();
    Ok(BenchReport {
        frame_ms: times[times.len() / 2],
        p90_ms: times[times.len() * 9 / 10],
        frames: FRAMES,
        width: WIDTH,
        height: HEIGHT,
        draw_calls: stats.draw_calls,
        triangles: stats.triangles,
        gpu: gpu.info(),
    })
}

/// Chase camera `t` (0–1) of the way down the middle avenue.
fn camera(t: f32) -> Camera {
    let x = -SPACING_M * 0.8 + t * SPACING_M * 1.6;
    Camera {
        eye: Vec3::new(x - 7.5, -2.6, 3.0),
        target: Vec3::new(x + 2.5, -2.6, 1.0),
        up: Vec3::Z,
        fov_y_deg: 55.0,
    }
}

fn roads() -> RoadNetwork {
    let id = |i: i32, j: i32| format!("n{i}{j}");
    let mut nodes = Vec::new();
    let mut roads = Vec::new();
    for i in -1..=1 {
        for j in -1..=1 {
            nodes.push(NodeDef {
                id: id(i, j),
                position: (i as f32 * SPACING_M, j as f32 * SPACING_M),
                control: Default::default(),
            });
            let mut road = |to: String| {
                roads.push(RoadDef {
                    id: format!("{}_{to}", id(i, j)),
                    name: None,
                    from: id(i, j),
                    to,
                    shape: RoadShape::Straight,
                    lanes_forward: 2,
                    lanes_backward: 2,
                    lane_width_m: 3.5,
                    speed_limit_kmh: 50.0,
                    sidewalk_m: 3.0,
                });
            };
            if i < 1 {
                road(id(i + 1, j));
            }
            if j < 1 {
                road(id(i, j + 1));
            }
        }
    }
    RoadNetwork::build(&RoadNetworkDef {
        name: "bench".into(),
        drive_side: DriveSide::Right,
        nodes,
        roads,
        stops: Vec::new(),
    })
}

/// The benchmark's frame (the camera is set per frame).
pub fn scene() -> FrameView {
    let net = Arc::new(roads());
    let scenery = Arc::new(Scenery::build(
        &SceneryDef {
            name: "bench".into(),
            streets: vec![StreetDef::default()],
            tree_rows: vec![TreeRowDef {
                spacing_m: 14.0,
                ..Default::default()
            }],
            ..Default::default()
        },
        Some(&net),
    ));
    let mut items = Vec::new();
    let mut lights = Vec::new();
    extract::tree_items(&mut items, &scenery, Vec2::ZERO);
    extract::lamp_items(&mut items, &mut lights, &net, Vec2::ZERO, true);
    // Cars in both directions along every road, headlights on.
    for (k, road) in net.roads.iter().enumerate() {
        let len = road.centre.length();
        for n in 0..2 {
            let fwd = n == 0;
            let (p, along) = road.centre.sample(len * (0.3 + 0.4 * ((k + n) % 2) as f32));
            let right = Vec2::new(along.y, -along.x);
            let dir = if fwd { along } else { -along };
            let lane = right * if fwd { 5.25 } else { -5.25 };
            car(&mut items, &mut lights, p + lane, dir, k * 2 + n);
        }
    }
    FrameView {
        camera: camera(0.0),
        environment: Environment::at_hour(22.0).with_rain(0.8),
        items,
        shadows: Vec::new(),
        roads: Some(net),
        scenery: Some(scenery),
        wetness: 1.0,
        rain: 0.8,
        time_s: 0.0,
        models: None,
        lights,
        deformed: Vec::new(),
        look: wp_world::Look::default(),
        windscreen: None,
    }
}

fn car(items: &mut Vec<DrawItem>, lights: &mut Vec<PointLight>, at: Vec2, dir: Vec2, seed: usize) {
    let heading = dir.y.atan2(dir.x);
    let base = Mat4::from_translation(at.extend(0.0)) * Mat4::from_rotation_z(heading);
    let paint = [
        Vec3::new(0.5, 0.05, 0.04),
        Vec3::new(0.05, 0.12, 0.4),
        Vec3::new(0.6, 0.6, 0.62),
        Vec3::new(0.03, 0.03, 0.03),
    ][seed % 4];
    for (centre, size) in [
        (Vec3::new(0.0, 0.0, 0.65), Vec3::new(4.2, 1.8, 0.8)),
        (Vec3::new(-0.3, 0.0, 1.3), Vec3::new(2.2, 1.6, 0.6)),
    ] {
        items.push(DrawItem {
            mesh: MeshId::Cube,
            transform: base * Mat4::from_translation(centre) * Mat4::from_scale(size),
            color: paint,
            roughness: 0.3,
            metallic: 0.5,
            emissive: 0.0,
        });
    }
    for side in [-0.6, 0.6] {
        for (x, y) in [(-1.3f32, 0.35f32), (1.3, 0.35)] {
            items.push(DrawItem {
                mesh: MeshId::Wheel,
                transform: base
                    * Mat4::from_translation(Vec3::new(x, side * 1.25, y))
                    * Mat4::from_scale(Vec3::new(0.65, 0.22, 0.65)),
                color: Vec3::splat(0.03),
                roughness: 0.9,
                metallic: 0.0,
                emissive: 0.0,
            });
        }
        let lamp = base.transform_point3(Vec3::new(2.2, side, 0.7));
        lights.push(PointLight {
            position: lamp,
            color: Vec3::new(1.0, 0.95, 0.85) * 160.0,
            range: 60.0,
            direction: (dir.extend(-0.08)).normalize(),
            cone_cos: 24f32.to_radians().cos(),
        });
        lights.push(PointLight {
            position: base.transform_point3(Vec3::new(-2.2, side, 0.8)),
            color: Vec3::new(1.0, 0.04, 0.02) * 0.8,
            range: 5.0,
            direction: Vec3::Z,
            cone_cos: -2.0,
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_scene_is_a_lit_town() {
        let f = super::scene();
        let scenery = f.scenery.as_ref().unwrap();
        assert!(scenery.buildings.len() > 100, "{}", scenery.buildings.len());
        assert!(scenery.trees.len() > 50, "{}", scenery.trees.len());
        assert!(f.lights.len() > 150, "{}", f.lights.len());
        assert!(f.environment.darkness() > 0.9);
    }
}

/// `WP_BENCH_PNG=<path> cargo test -p wp_render bench_picture -- --ignored`:
/// save one frame of the benchmark to look at.
#[cfg(test)]
mod picture {
    #[test]
    #[ignore]
    fn bench_picture() {
        let Ok(path) = std::env::var("WP_BENCH_PNG") else {
            return;
        };
        let gpu = wp_rhi::Gpu::headless().unwrap();
        let mut frame = super::scene();
        frame.camera = super::camera(0.5);
        let px = crate::render_rgba(&gpu, &frame, super::WIDTH, super::HEIGHT).unwrap();
        wp_rhi::write_png(
            std::path::Path::new(&path),
            super::WIDTH,
            super::HEIGHT,
            &px,
        )
        .unwrap();
    }
}
