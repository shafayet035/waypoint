//! Upscaling quality: the town drawn at 2/3 of the output size each way
//! (as Low draws 720p for a 1080p screen) and scaled up, judged against a
//! supersampled picture of the same frame (drawn at twice the output size
//! each way and averaged down). Skips itself without a GPU.
//! WP_RENDER_OUT=<dir> keeps the pictures.

use glam::Vec3;
use wp_render::{Environment, FrameView, render_rgba};

const W: u32 = 480;
const H: u32 = 270;

fn linear(b: u8) -> f32 {
    let c = b as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// `rgba` (2W × 2H) averaged down to W × H, linear RGB.
fn downsample(rgba: &[u8]) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0; 3]; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let mut c = [0.0; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (((y * 2 + dy) * W * 2 + x * 2 + dx) * 4) as usize;
                for (k, v) in c.iter_mut().enumerate() {
                    *v += linear(rgba[i + k]) / 4.0;
                }
            }
            out[(y * W + x) as usize] = c;
        }
    }
    out
}

/// Root-mean-square difference from the reference, linear RGB.
fn rms(rgba: &[u8], reference: &[[f32; 3]]) -> f32 {
    let mut sum = 0.0;
    for (i, r) in reference.iter().enumerate() {
        for (k, v) in r.iter().enumerate() {
            let d = linear(rgba[i * 4 + k]) - v;
            sum += d * d;
        }
    }
    (sum / (reference.len() * 3) as f32).sqrt()
}

/// The benchmark town by day: buildings, trees, lamp posts, cars.
fn town() -> FrameView {
    let mut f = wp_render::bench::scene();
    f.environment = Environment::default();
    f.rain = 0.0;
    f.wetness = 0.0;
    f.camera = wp_render::Camera {
        eye: Vec3::new(-47.5, -2.6, 3.0),
        target: Vec3::new(-37.5, -2.6, 1.0),
        up: Vec3::Z,
        fov_y_deg: 55.0,
    };
    f
}

#[test]
fn upscaled_frames_stay_close_to_full_resolution() {
    let gpu = match wp_rhi::Gpu::headless() {
        Ok(g) => g,
        Err(_) => {
            eprintln!("no GPU: skipping");
            return;
        }
    };
    let frame = town();
    let reference = downsample(&render_rgba(&gpu, &frame, W * 2, H * 2).unwrap());
    let full = render_rgba(&gpu, &frame, W, H).unwrap();
    let target = wp_rhi::OffscreenTarget::new(&gpu, W, H);
    let mut r = wp_render::Renderer::new(&gpu, wp_rhi::OFFSCREEN_FORMAT, W, H);
    r.set_max_height(&gpu, Some(H * 2 / 3));
    r.render(&gpu, &frame, &target.view);
    assert_eq!(r.internal_size(), (W * 2 / 3, H * 2 / 3));
    let up = target.read_rgba(&gpu).unwrap();
    if let Ok(dir) = std::env::var("WP_RENDER_OUT") {
        let dir = std::path::Path::new(&dir);
        wp_rhi::write_png(&dir.join("upscale_full.png"), W, H, &full).unwrap();
        wp_rhi::write_png(&dir.join("upscale_two_thirds.png"), W, H, &up).unwrap();
    }
    let (e_full, e_up) = (rms(&full, &reference), rms(&up, &reference));
    eprintln!("full resolution {e_full:.5}, upscaled from 2/3 {e_up:.5}");
    // Plain bilinear stretching scores about 0.0351 here.
    assert!(e_up < 0.032, "upscaled {e_up} (full resolution {e_full})");
}
