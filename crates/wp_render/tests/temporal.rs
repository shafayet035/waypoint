//! Temporal anti-aliasing and upscaling, judged against a supersampled
//! picture of the same frame (drawn at twice the size each way, averaged
//! down). Over a few frames TAA must come closer to it than a single frame
//! does, at full resolution and when upscaling from a lower one, and a
//! moving camera with a car moving along with it must not leave ghosts.
//! Skips itself without a GPU. WP_RENDER_OUT=<dir> keeps the pictures.

use glam::{Mat4, Vec3};
use wp_render::{DrawItem, Environment, FrameView, MeshId, render_rgba, render_rgba_sequence};

const W: u32 = 480;
const H: u32 = 270;

fn gpu() -> Option<wp_rhi::Gpu> {
    match wp_rhi::Gpu::headless() {
        Ok(g) => Some(g),
        Err(_) => {
            eprintln!("no GPU: skipping");
            None
        }
    }
}

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
                for k in 0..3 {
                    c[k] += linear(rgba[i + k]) / 4.0;
                }
            }
            out[(y * W + x) as usize] = c;
        }
    }
    out
}

/// Root-mean-square difference from the reference, in linear RGB.
fn rms(rgba: &[u8], reference: &[[f32; 3]]) -> f32 {
    let mut sum = 0.0;
    for (i, r) in reference.iter().enumerate() {
        for k in 0..3 {
            let d = linear(rgba[i * 4 + k]) - r[k];
            sum += d * d;
        }
    }
    (sum / (reference.len() * 3) as f32).sqrt()
}

fn save(name: &str, rgba: &[u8], w: u32, h: u32) {
    if let Ok(dir) = std::env::var("WP_RENDER_OUT") {
        wp_rhi::write_png(&std::path::Path::new(&dir).join(name), w, h, rgba).unwrap();
    }
}

/// The benchmark town by day: buildings, trees, lamp posts, cars.
fn town(t: f32) -> FrameView {
    let mut f = wp_render::bench::scene();
    f.environment = Environment::default();
    f.rain = 0.0;
    f.wetness = 0.0;
    let x = -40.0 + t * 12.0;
    f.camera = wp_render::Camera {
        eye: Vec3::new(x - 7.5, -2.6, 3.0),
        target: Vec3::new(x + 2.5, -2.6, 1.0),
        up: Vec3::Z,
        fov_y_deg: 55.0,
    };
    f
}

#[test]
fn still_frames_converge_towards_supersampling() {
    let Some(gpu) = gpu() else { return };
    let frame = town(0.0);
    let reference = downsample(&render_rgba(&gpu, &frame, W * 2, H * 2).unwrap());
    let single = render_rgba(&gpu, &frame, W, H).unwrap();
    let frames = vec![frame.clone(); 16];
    let taa = render_rgba_sequence(&gpu, &frames, W, H, None, true).unwrap();
    save("taa_still_single.png", &single, W, H);
    save("taa_still.png", &taa, W, H);
    let (e_single, e_taa) = (rms(&single, &reference), rms(&taa, &reference));
    eprintln!("still: single frame {e_single:.5}, TAA {e_taa:.5}");
    assert!(e_taa < e_single * 0.9, "TAA {e_taa} vs single {e_single}");
}

#[test]
fn upscaling_beats_stretching() {
    let Some(gpu) = gpu() else { return };
    // Internal 2/3 of the output each way (as Low draws 720p for 1080p).
    let frame = town(0.0);
    let reference = downsample(&render_rgba(&gpu, &frame, W * 2, H * 2).unwrap());
    let render = |temporal: bool| {
        let target = wp_rhi::OffscreenTarget::new(&gpu, W, H);
        let mut r = wp_render::Renderer::new(&gpu, wp_rhi::OFFSCREEN_FORMAT, W, H);
        r.set_max_height(&gpu, Some(H * 2 / 3));
        r.set_temporal(&gpu, temporal);
        for _ in 0..16 {
            r.render(&gpu, &frame, &target.view);
        }
        assert_eq!(r.internal_size(), (W * 2 / 3, H * 2 / 3));
        target.read_rgba(&gpu).unwrap()
    };
    let (stretched, taa) = (render(false), render(true));
    save("taa_up_stretched.png", &stretched, W, H);
    save("taa_up.png", &taa, W, H);
    let (e_stretched, e_taa) = (rms(&stretched, &reference), rms(&taa, &reference));
    eprintln!("upscaling: bilinear {e_stretched:.5}, TAA {e_taa:.5}");
    assert!(
        e_taa < e_stretched * 0.85,
        "TAA {e_taa} vs bilinear {e_stretched}"
    );
}

/// A red box that rides along with the camera, like the player's car in
/// the chase view: mover 1.
fn with_car(mut f: FrameView) -> FrameView {
    let at = f.camera.eye + Vec3::new(7.5, 0.0, -3.0);
    let body = Mat4::from_translation(at);
    f.items.push(DrawItem {
        mesh: MeshId::Cube,
        transform: body
            * Mat4::from_scale_rotation_translation(
                Vec3::new(4.2, 1.8, 1.2),
                glam::Quat::IDENTITY,
                Vec3::new(0.0, 0.0, 0.7),
            ),
        color: Vec3::new(0.6, 0.04, 0.03),
        roughness: 0.4,
        metallic: 0.0,
        emissive: 0.0,
        mover: 1,
    });
    f.movers.push((1, body));
    f
}

#[test]
fn moving_camera_and_car_leave_no_ghosts() {
    let Some(gpu) = gpu() else { return };
    // 16 frames at 60 fps, driving at 12 m/s with the car in front.
    let frames: Vec<FrameView> = (0..16).map(|i| with_car(town(i as f32 / 60.0))).collect();
    let last = frames.last().unwrap().clone();
    let reference = downsample(&render_rgba(&gpu, &last, W * 2, H * 2).unwrap());
    let single = render_rgba(&gpu, &last, W, H).unwrap();
    let taa = render_rgba_sequence(&gpu, &frames, W, H, None, true).unwrap();
    save("taa_moving_single.png", &single, W, H);
    save("taa_moving.png", &taa, W, H);
    let (e_single, e_taa) = (rms(&single, &reference), rms(&taa, &reference));
    eprintln!("moving: single frame {e_single:.5}, TAA {e_taa:.5}");
    // Where TAA differs from the reference (white: a lot).
    let diff: Vec<u8> = reference
        .iter()
        .enumerate()
        .flat_map(|(i, r)| {
            let d: f32 = (0..3).map(|k| (linear(taa[i * 4 + k]) - r[k]).abs()).sum();
            let v = (d * 4.0 * 255.0).min(255.0) as u8;
            [v, v, v, 255]
        })
        .collect();
    save("taa_moving_diff.png", &diff, W, H);
    for (dx, dy) in [(0i32, 0i32), (1, 0), (-1, 0), (0, 1), (0, -1)] {
        let mut sum = 0.0;
        let mut n = 0;
        for y in 2..H as i32 - 2 {
            for x in 2..W as i32 - 2 {
                let i = (y * W as i32 + x) as usize;
                let j = ((y + dy) * W as i32 + x + dx) as usize;
                for k in 0..3 {
                    let d = linear(taa[j * 4 + k]) - reference[i][k];
                    sum += d * d;
                    n += 1;
                }
            }
        }
        eprintln!("shift ({dx},{dy}): {:.5}", (sum / n as f32).sqrt());
    }
    // Ghosts or smearing would leave it far from the reference.
    assert!(e_taa < e_single * 1.05, "TAA {e_taa} vs single {e_single}");
    // The car itself stays sharp: compare a box round it.
    let car_rms = |rgba: &[u8]| {
        let (x0, x1, y0, y1) = (W / 2 - 60, W / 2 + 60, H / 2 - 10, H / 2 + 60);
        let mut sum = 0.0;
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * W + x) as usize;
                for k in 0..3 {
                    let d = linear(rgba[i * 4 + k]) - reference[i][k];
                    sum += d * d;
                    n += 1;
                }
            }
        }
        (sum / n as f32).sqrt()
    };
    let (c_single, c_taa) = (car_rms(&single), car_rms(&taa));
    eprintln!("car: single frame {c_single:.5}, TAA {c_taa:.5}");
    assert!(
        c_taa < c_single * 1.1,
        "car TAA {c_taa} vs single {c_single}"
    );
}

#[test]
fn upscaling_in_motion_beats_stretching() {
    let Some(gpu) = gpu() else { return };
    let frames: Vec<FrameView> = (0..16).map(|i| with_car(town(i as f32 / 60.0))).collect();
    let last = frames.last().unwrap().clone();
    let reference = downsample(&render_rgba(&gpu, &last, W * 2, H * 2).unwrap());
    let render = |temporal: bool| {
        let target = wp_rhi::OffscreenTarget::new(&gpu, W, H);
        let mut r = wp_render::Renderer::new(&gpu, wp_rhi::OFFSCREEN_FORMAT, W, H);
        r.set_max_height(&gpu, Some(H * 2 / 3));
        r.set_temporal(&gpu, temporal);
        for f in &frames {
            r.render(&gpu, f, &target.view);
        }
        target.read_rgba(&gpu).unwrap()
    };
    let (stretched, taa) = (render(false), render(true));
    save("taa_up_moving_stretched.png", &stretched, W, H);
    save("taa_up_moving.png", &taa, W, H);
    let (e_stretched, e_taa) = (rms(&stretched, &reference), rms(&taa, &reference));
    eprintln!("upscaling in motion: bilinear {e_stretched:.5}, TAA {e_taa:.5}");
    assert!(e_taa < e_stretched, "TAA {e_taa} vs bilinear {e_stretched}");
}
