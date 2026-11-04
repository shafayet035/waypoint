//! Comparing rendered pictures with approved ones, for screenshot tests.
//!
//! Pixels are compared in Oklab, where equal distances look about equally
//! different (0.02 is about the smallest difference people notice). Each
//! pixel is matched against the 3×3 neighbourhood of the other picture, both
//! ways, so edges that moved by one pixel (anti-aliasing on another GPU) do
//! not count as changes.

use serde::Serialize;

/// How two pictures differ.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct ImageDiff {
    /// Share of pixels that changed by more than the threshold, %.
    pub changed_pct: f32,
    /// Mean and largest difference (Oklab distance; 0.02 is just noticeable).
    pub mean: f32,
    pub max: f32,
    /// Bounds of the changed pixels: x0, y0, x1, y1 (inclusive).
    pub changed_area: Option<[u32; 4]>,
}

fn srgb_to_linear(c: u8) -> f32 {
    let x = c as f32 / 255.0;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB bytes → Oklab (L, a, b).
pub fn oklab(rgb: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn to_lab(rgba: &[u8]) -> Vec<[f32; 3]> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| oklab([p[0], p[1], p[2]]))
        .collect()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Per pixel: the smallest difference to any pixel in the other picture's
/// 3×3 neighbourhood, taken both ways (the larger of the two).
fn differences(a: &[u8], b: &[u8], width: u32, height: u32) -> Vec<f32> {
    let (la, lb) = (to_lab(a), to_lab(b));
    let (w, h) = (width as i64, height as i64);
    let nearest = |from: &[[f32; 3]], to: &[[f32; 3]], x: i64, y: i64| {
        let p = from[(y * w + x) as usize];
        let mut best = f32::MAX;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (qx, qy) = (x + dx, y + dy);
                if qx < 0 || qy < 0 || qx >= w || qy >= h {
                    continue;
                }
                best = best.min(dist(p, to[(qy * w + qx) as usize]));
            }
        }
        best
    };
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            out.push(nearest(&la, &lb, x, y).max(nearest(&lb, &la, x, y)));
        }
    }
    out
}

/// Compare two RGBA8 pictures of the same size; pixels further apart than
/// `threshold` (Oklab distance) count as changed.
pub fn compare(a: &[u8], b: &[u8], width: u32, height: u32, threshold: f32) -> ImageDiff {
    let d = differences(a, b, width, height);
    let mut diff = ImageDiff::default();
    let mut changed = 0usize;
    let mut area = [u32::MAX, u32::MAX, 0, 0];
    for (i, v) in d.iter().enumerate() {
        diff.mean += v;
        diff.max = diff.max.max(*v);
        if *v > threshold {
            changed += 1;
            let (x, y) = (i as u32 % width, i as u32 / width);
            area = [
                area[0].min(x),
                area[1].min(y),
                area[2].max(x),
                area[3].max(y),
            ];
        }
    }
    diff.mean /= d.len().max(1) as f32;
    diff.changed_pct = changed as f32 * 100.0 / d.len().max(1) as f32;
    diff.changed_area = (changed > 0).then_some(area);
    diff
}

/// A picture of where `actual` differs from `approved`: the actual picture
/// faded to grey, with changed pixels in magenta (brighter for bigger changes).
pub fn diff_image(
    approved: &[u8],
    actual: &[u8],
    width: u32,
    height: u32,
    threshold: f32,
) -> Vec<u8> {
    let d = differences(approved, actual, width, height);
    let mut out = Vec::with_capacity(actual.len());
    for (p, v) in actual.as_chunks::<4>().0.iter().zip(d) {
        if v > threshold {
            let k = (0.5 + v * 4.0).min(1.0);
            out.extend_from_slice(&[(255.0 * k) as u8, 0, (255.0 * k) as u8, 255]);
        } else {
            let grey = ((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3 / 3 + 150) as u8;
            out.extend_from_slice(&[grey, grey, grey, 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                v.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        v
    }

    #[test]
    fn identical_pictures_do_not_differ() {
        let a = picture(32, 16, |x, y| [(x * 8) as u8, (y * 16) as u8, 90]);
        let d = compare(&a, &a, 32, 16, 0.02);
        assert_eq!(d.changed_pct, 0.0);
        assert_eq!(d.max, 0.0);
        assert!(d.changed_area.is_none());
    }

    #[test]
    fn edges_moved_by_one_pixel_are_tolerated() {
        let edge = |at: u32| {
            picture(32, 16, move |x, _| {
                if x < at {
                    [20, 20, 20]
                } else {
                    [230, 230, 230]
                }
            })
        };
        let d = compare(&edge(16), &edge(17), 32, 16, 0.02);
        assert_eq!(d.changed_pct, 0.0, "{d:?}");
        let d = compare(&edge(16), &edge(19), 32, 16, 0.02);
        assert!(d.changed_pct > 5.0, "{d:?}");
    }

    #[test]
    fn a_changed_patch_is_found() {
        let a = picture(32, 16, |_, _| [120, 140, 160]);
        let b = picture(32, 16, |x, y| {
            if (10..14).contains(&x) && (4..8).contains(&y) {
                [200, 60, 60]
            } else {
                [120, 140, 160]
            }
        });
        let d = compare(&a, &b, 32, 16, 0.02);
        // Every pixel of the new patch has no close match in the old picture.
        assert_eq!(d.changed_area, Some([10, 4, 13, 7]));
        assert!((d.changed_pct - 16.0 * 100.0 / 512.0).abs() < 1e-3);
        assert!(d.max > 0.2);
        let img = diff_image(&a, &b, 32, 16, 0.02);
        assert_eq!(&img[(5 * 32 + 11) * 4..(5 * 32 + 11) * 4 + 2], &[255, 0]);
    }

    #[test]
    fn oklab_of_white_and_black() {
        let w = oklab([255, 255, 255]);
        assert!((w[0] - 1.0).abs() < 1e-3 && w[1].abs() < 1e-3 && w[2].abs() < 1e-3);
        assert!(oklab([0, 0, 0])[0].abs() < 1e-6);
    }
}
