//! Street reflections in wet roads: the scene mirrored in the road plane.
//! A wet street must show what stands on it (not just the sky), and a dry
//! one must be drawn exactly as without reflections. Skips without a GPU.
//! WP_RENDER_OUT=<dir> keeps the pictures.

use glam::Vec3;
use wp_render::{Environment, FrameView, Renderer};

const W: u32 = 480;
const H: u32 = 270;

fn town(wetness: f32) -> FrameView {
    let mut f = wp_render::bench::scene();
    f.environment = Environment::default();
    f.rain = 0.0;
    f.wetness = wetness;
    f.camera = wp_render::Camera {
        eye: Vec3::new(-47.5, -2.6, 3.0),
        target: Vec3::new(-37.5, -2.6, 1.0),
        up: Vec3::Z,
        fov_y_deg: 55.0,
    };
    f
}

fn draw(gpu: &wp_rhi::Gpu, frame: &FrameView, reflections: bool) -> Vec<u8> {
    let target = wp_rhi::OffscreenTarget::new(gpu, W, H);
    let mut r = Renderer::new(gpu, wp_rhi::OFFSCREEN_FORMAT, W, H);
    if !reflections {
        r.set_reflections(gpu, None);
    }
    r.render(gpu, frame, &target.view);
    target.read_rgba(gpu).unwrap()
}

#[test]
fn wet_roads_mirror_the_street_and_dry_ones_are_untouched() {
    let gpu = match wp_rhi::Gpu::headless() {
        Ok(g) => g,
        Err(_) => {
            eprintln!("no GPU: skipping");
            return;
        }
    };
    let dry = town(0.0);
    assert!(
        draw(&gpu, &dry, true) == draw(&gpu, &dry, false),
        "dry frames changed"
    );

    let wet = town(1.0);
    let (with, without) = (draw(&gpu, &wet, true), draw(&gpu, &wet, false));
    if let Ok(dir) = std::env::var("WP_RENDER_OUT") {
        let dir = std::path::Path::new(&dir);
        wp_rhi::write_png(&dir.join("reflect_on.png"), W, H, &with).unwrap();
        wp_rhi::write_png(&dir.join("reflect_off.png"), W, H, &without).unwrap();
    }
    // The road in the lower half shows the buildings and trees along the
    // street where it used to show only the sky: a clear change, pixel by pixel.
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in H * 3 / 5..H {
        for x in W / 8..W * 7 / 8 {
            let i = ((y * W + x) * 4) as usize;
            for k in 0..3 {
                sum += (with[i + k] as f32 - without[i + k] as f32).abs();
                n += 1.0;
            }
        }
    }
    let change = sum / n;
    eprintln!("wet road: mean change {change:.1} levels of 255");
    assert!(
        change > 4.0,
        "reflections changed the road by only {change}"
    );
}
