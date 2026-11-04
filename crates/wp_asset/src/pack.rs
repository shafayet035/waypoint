//! Shipping models: images inside a `.glb` reduced to a quality tier's
//! texture size and re-encoded, so a phone never downloads detail it would
//! throw away.
//!
//! Compressed (the default for `wp asset build`): every image becomes a
//! `.basis` file (mime type `image/x-basis`), ETC1S for colour maps and
//! UASTC for normal and data maps, which the phone transcodes to ASTC, ETC2
//! or BC7 and keeps compressed in memory. Otherwise colour maps without
//! transparency become JPEG and data maps stay lossless PNG.

use serde_json::{Value, json};
use std::collections::BTreeSet;

/// What packing did to one model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackStats {
    pub bytes_in: usize,
    pub bytes_out: usize,
    pub images: usize,
    /// Images made smaller than they were.
    pub resized: usize,
}

const JPEG_QUALITY: u8 = 85;

fn split_glb(bytes: &[u8]) -> Result<(Value, Vec<u8>), String> {
    let u32_at = |i: usize| -> Result<u32, String> {
        bytes
            .get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "truncated GLB".to_string())
    };
    if bytes.get(0..4) != Some(b"glTF") || u32_at(4)? != 2 {
        return Err("not a glTF 2.0 binary (.glb)".into());
    }
    let json_len = u32_at(12)? as usize;
    if bytes.get(16..20) != Some(b"JSON") {
        return Err("the GLB's first chunk is not JSON".into());
    }
    let json: Value =
        serde_json::from_slice(bytes.get(20..20 + json_len).ok_or("truncated JSON chunk")?)
            .map_err(|e| e.to_string())?;
    let at = 20 + json_len;
    let bin = if bytes.len() >= at + 8 && &bytes[at + 4..at + 8] == b"BIN\0" {
        let len = u32_at(at)? as usize;
        bytes
            .get(at + 8..at + 8 + len)
            .ok_or("truncated BIN chunk")?
            .to_vec()
    } else {
        Vec::new()
    };
    Ok((json, bin))
}

fn join_glb(json: &Value, bin: &[u8]) -> Vec<u8> {
    let mut js = serde_json::to_vec(json).expect("JSON serialises");
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }
    let mut bin = bin.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + js.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&js);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
    }
    out
}

/// What each image holds, by image index: normal maps, and other data
/// maps (metallic-roughness, occlusion). Anything else is colour.
fn image_kinds(json: &Value) -> (BTreeSet<usize>, BTreeSet<usize>) {
    let texture_source = |t: &Value| -> Option<usize> {
        let ti = t.get("index")?.as_u64()? as usize;
        json.get("textures")?
            .get(ti)?
            .get("source")?
            .as_u64()
            .map(|s| s as usize)
    };
    let (mut normal, mut data) = (BTreeSet::new(), BTreeSet::new());
    for m in json
        .get("materials")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        normal.extend(m.get("normalTexture").and_then(texture_source));
        let maps = [
            m.get("occlusionTexture"),
            m.get("pbrMetallicRoughness")
                .and_then(|p| p.get("metallicRoughnessTexture")),
        ];
        data.extend(maps.into_iter().flatten().filter_map(texture_source));
    }
    (normal, data)
}

/// Reduce an image and encode it as `.basis`, at whole 4×4 blocks.
fn compress(bytes: &[u8], max_px: u32, kind: crate::basis::ImageKind) -> Option<(Vec<u8>, bool)> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let scale = (max_px as f32 / w.max(h) as f32).min(1.0);
    let fit = |v: u32| (((v as f32 * scale) as u32) / 4 * 4).max(4);
    let (nw, nh) = (fit(w), fit(h));
    let resized = nw < w || nh < h;
    let img = if (nw, nh) != (w, h) {
        img.resize_exact(nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let out = crate::basis::encode(rgba.as_raw(), nw, nh, kind).ok()?;
    Some((out, resized))
}

/// Reduce and re-encode one image. `None` keeps the original bytes.
fn shrink(bytes: &[u8], max_px: u32, lossless: bool) -> Option<(Vec<u8>, &'static str, bool)> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let resized = w.max(h) > max_px;
    let img = if resized {
        img.resize(max_px, max_px, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let opaque = rgba.pixels().all(|p| p.0[3] == 255);
    let mut out = std::io::Cursor::new(Vec::new());
    let mime = if !lossless && opaque {
        let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
            .encode_image(&rgb)
            .ok()?;
        "image/jpeg"
    } else {
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut out, image::ImageFormat::Png)
            .ok()?;
        "image/png"
    };
    let out = out.into_inner();
    // Keep the original if re-encoding did not help and nothing shrank.
    if !resized && out.len() >= bytes.len() {
        return None;
    }
    Some((out, mime, resized))
}

/// Pack a `.glb` for a tier whose largest texture side is `max_px`.
/// Models without embedded images come back unchanged.
/// With `gpu_compress`, images become `.basis` (see the module docs).
pub fn pack_glb(
    bytes: &[u8],
    max_px: u32,
    gpu_compress: bool,
) -> Result<(Vec<u8>, PackStats), String> {
    let mut stats = PackStats {
        bytes_in: bytes.len(),
        ..Default::default()
    };
    let (mut json, bin) = split_glb(bytes)?;
    let (normals, data) = image_kinds(&json);
    let views = json
        .get("bufferViews")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // New bytes for image buffer views, by view index.
    let mut replaced: Vec<Option<Vec<u8>>> = vec![None; views.len()];
    let images = json
        .get("images")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (i, img) in images.iter().enumerate() {
        let Some(v) = img
            .get("bufferView")
            .and_then(Value::as_u64)
            .map(|v| v as usize)
        else {
            continue;
        };
        let Some(view) = views.get(v) else { continue };
        let off = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let len = view.get("byteLength").and_then(Value::as_u64).unwrap_or(0) as usize;
        let Some(src) = bin.get(off..off + len) else {
            continue;
        };
        stats.images += 1;
        if crate::basis::is_basis(src) {
            continue; // already packed
        }
        if gpu_compress {
            let kind = if normals.contains(&i) {
                crate::basis::ImageKind::Normal
            } else if data.contains(&i) {
                crate::basis::ImageKind::Data
            } else {
                crate::basis::ImageKind::Colour
            };
            if let Some((out, resized)) = compress(src, max_px, kind) {
                stats.resized += resized as usize;
                replaced[v] = Some(out);
                json["images"][i]["mimeType"] = json!("image/x-basis");
            }
        } else if let Some((out, mime, resized)) =
            shrink(src, max_px, normals.contains(&i) || data.contains(&i))
        {
            stats.resized += resized as usize;
            replaced[v] = Some(out);
            json["images"][i]["mimeType"] = json!(mime);
        }
    }
    if replaced.iter().all(Option::is_none) {
        stats.bytes_out = bytes.len();
        return Ok((bytes.to_vec(), stats));
    }
    // Lay the buffer out again, view by view (4-byte aligned).
    let mut out_bin = Vec::with_capacity(bin.len());
    for (k, view) in views.iter().enumerate() {
        while !out_bin.len().is_multiple_of(4) {
            out_bin.push(0);
        }
        let off = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let len = view.get("byteLength").and_then(Value::as_u64).unwrap_or(0) as usize;
        let data: &[u8] = match &replaced[k] {
            Some(d) => d,
            None => bin
                .get(off..off + len)
                .ok_or("a buffer view runs past the buffer")?,
        };
        json["bufferViews"][k]["byteOffset"] = json!(out_bin.len());
        json["bufferViews"][k]["byteLength"] = json!(data.len());
        out_bin.extend_from_slice(data);
    }
    json["buffers"][0]["byteLength"] = json!(out_bin.len());
    let out = join_glb(&json, &out_bin);
    stats.bytes_out = out.len();
    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model with a big noisy base colour image and a big normal map.
    fn big_textured() -> Vec<u8> {
        let png = |w: u32, f: &dyn Fn(u32, u32) -> [u8; 4]| {
            let img = image::RgbaImage::from_fn(w, w, |x, y| image::Rgba(f(x, y)));
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut out, image::ImageFormat::Png)
                .unwrap();
            out.into_inner()
        };
        let noise = |x: u32, y: u32| {
            let v = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)) as u8;
            [v, v / 2, 255 - v, 255]
        };
        let base = png(2048, &noise);
        let normal = png(2048, &|x, y| {
            [128 + (x % 7) as u8, 128 + (y % 5) as u8, 250, 255]
        });
        let pts: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]];
        let uvs: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let mut bin: Vec<u8> = pts.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        bin.extend(uvs.iter().flatten().flat_map(|f| f.to_le_bytes()));
        let base_at = bin.len();
        bin.extend(&base);
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let normal_at = bin.len();
        bin.extend(&normal);
        let json = json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}], "scene": 0,
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "material": 0}]}],
            "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}, "normalTexture": {"index": 1}}],
            "textures": [{"source": 0}, {"source": 1}],
            "images": [{"bufferView": 2, "mimeType": "image/png"}, {"bufferView": 3, "mimeType": "image/png"}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,1]},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 24},
                {"buffer": 0, "byteOffset": base_at, "byteLength": base.len()},
                {"buffer": 0, "byteOffset": normal_at, "byteLength": normal.len()}
            ],
            "buffers": [{"byteLength": bin.len()}]
        });
        join_glb(&json, &bin)
    }

    #[test]
    fn packing_shrinks_images_and_keeps_the_model() {
        let original = big_textured();
        let (packed, stats) = pack_glb(&original, 512, false).unwrap();
        assert_eq!((stats.images, stats.resized), (2, 2));
        assert!(
            packed.len() * 4 < original.len(),
            "{} → {} bytes",
            original.len(),
            packed.len()
        );
        let m = crate::parse_model(&packed, "p.glb", None).unwrap();
        let mat = &m.parts[0].material;
        let base = mat.base_color_texture.as_ref().unwrap();
        assert_eq!((base.width, base.height), (512, 512));
        let (normal, _) = mat.normal_texture.as_ref().unwrap();
        assert_eq!(normal.width, 512);
        let (json, _) = split_glb(&packed).unwrap();
        assert_eq!(json["images"][0]["mimeType"], "image/jpeg", "colour → JPEG");
        assert_eq!(
            json["images"][1]["mimeType"], "image/png",
            "normal map stays lossless"
        );
        assert_eq!(m.triangles, 1);
        // A model with no images is passed through untouched.
        let plain = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/sandbox/assets/vehicles/hatchback.glb"),
        )
        .unwrap();
        assert_eq!(pack_glb(&plain, 512, false).unwrap().0, plain);
    }

    #[test]
    fn gpu_compressed_packing_ships_basis_images() {
        let original = big_textured();
        let (packed, stats) = pack_glb(&original, 256, true).unwrap();
        assert_eq!(stats.resized, 2);
        assert!(
            packed.len() * 8 < original.len(),
            "{} → {} bytes",
            original.len(),
            packed.len()
        );
        let (json, _) = split_glb(&packed).unwrap();
        assert_eq!(json["images"][0]["mimeType"], "image/x-basis");
        let m = crate::parse_model(&packed, "p.glb", None).unwrap();
        let base = m.parts[0].material.base_color_texture.as_ref().unwrap();
        assert!(
            base.basis.is_some() && base.rgba.is_empty(),
            "kept compressed"
        );
        assert_eq!((base.width, base.height), (256, 256));
        assert_eq!(
            base.pixels().unwrap().len(),
            256 * 256 * 4,
            "decodable on the CPU too"
        );
        // Packing again leaves it alone.
        assert_eq!(pack_glb(&packed, 256, true).unwrap().0.len(), packed.len());
    }
}
