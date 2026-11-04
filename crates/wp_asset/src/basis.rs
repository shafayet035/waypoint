//! GPU-compressed textures via Basis Universal.
//!
//! `wp asset build` encodes shipping images to `.basis` (ETC1S for colour
//! maps: small; UASTC for normal and data maps: higher quality), with their
//! mipmaps. On the phone each image is transcoded to what its GPU reads
//! directly (ASTC 4×4, ETC2 or BC7), so textures stay compressed in memory,
//! about a quarter of plain RGBA. A GPU with none of those gets RGBA.

use basis_universal::{
    BasisTextureFormat, ColorSpace, Compressor, CompressorParams, TranscodeParameters, Transcoder,
    TranscoderTextureFormat,
};

/// What an image holds, which decides how it is encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// Base colour or emissive (sRGB).
    Colour,
    /// Tangent-space normal map.
    Normal,
    /// Metallic-roughness or occlusion (linear).
    Data,
}

/// A format a GPU can sample directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuFormat {
    Astc4x4,
    Etc2Rgba,
    Bc7,
    /// Uncompressed, for GPUs without any of the above.
    Rgba8,
}

impl GpuFormat {
    /// Pixels per block side (1 for uncompressed).
    pub fn block(self) -> u32 {
        if self == GpuFormat::Rgba8 { 1 } else { 4 }
    }

    /// Bytes per block (per pixel when uncompressed).
    pub fn block_bytes(self) -> u32 {
        if self == GpuFormat::Rgba8 { 4 } else { 16 }
    }

    fn transcoder(self) -> TranscoderTextureFormat {
        match self {
            GpuFormat::Astc4x4 => TranscoderTextureFormat::ASTC_4x4_RGBA,
            GpuFormat::Etc2Rgba => TranscoderTextureFormat::ETC2_RGBA,
            GpuFormat::Bc7 => TranscoderTextureFormat::BC7_RGBA,
            GpuFormat::Rgba8 => TranscoderTextureFormat::RGBA32,
        }
    }
}

/// One mip level of a transcoded image.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Whether `bytes` is a `.basis` file.
pub fn is_basis(bytes: &[u8]) -> bool {
    bytes.starts_with(b"sB")
}

/// Encode RGBA8 pixels (rows top to bottom) with mipmaps.
pub fn encode(rgba: &[u8], width: u32, height: u32, kind: ImageKind) -> Result<Vec<u8>, String> {
    if rgba.len() != (width * height * 4) as usize || width == 0 || height == 0 {
        return Err(format!("{width}×{height} image with {} bytes", rgba.len()));
    }
    basis_universal::encoder_init();
    let mut params = CompressorParams::new();
    params.set_generate_mipmaps(true);
    match kind {
        ImageKind::Colour => {
            params.set_basis_format(BasisTextureFormat::ETC1S);
            params.set_etc1s_quality_level(basis_universal::ETC1S_QUALITY_DEFAULT);
            params.set_color_space(ColorSpace::Srgb);
        }
        ImageKind::Normal | ImageKind::Data => {
            params.set_basis_format(BasisTextureFormat::UASTC4x4);
            params.set_uastc_quality_level(basis_universal::UASTC_QUALITY_DEFAULT);
            params.set_color_space(ColorSpace::Linear);
            if kind == ImageKind::Normal {
                params.tune_for_normal_maps();
            }
        }
    }
    params.set_print_status_to_stdout(false);
    params.source_image_mut(0).init(rgba, width, height, 4);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    let mut compressor = Compressor::new(threads);
    // Safety: the parameters (and the image data they copied) outlive the
    // compression, which is how the library requires them to be used.
    unsafe {
        if !compressor.init(&params) {
            return Err("the Basis encoder refused the parameters".into());
        }
        compressor
            .process()
            .map_err(|e| format!("Basis encoding failed: {e:?}"))?;
    }
    Ok(compressor.basis_file().to_vec())
}

/// Width, height and mip levels of a `.basis` file.
pub fn info(bytes: &[u8]) -> Option<(u32, u32, u32)> {
    basis_universal::transcoder_init();
    let t = Transcoder::new();
    let i = t.image_info(bytes, 0)?;
    Some((i.m_orig_width, i.m_orig_height, i.m_total_levels))
}

/// Every mip level of the first image, in `format`.
pub fn transcode(bytes: &[u8], format: GpuFormat) -> Result<Vec<Level>, String> {
    basis_universal::transcoder_init();
    let mut t = Transcoder::new();
    t.prepare_transcoding(bytes)
        .map_err(|_| "not a valid .basis file".to_string())?;
    let levels = t.image_level_count(bytes, 0);
    let mut out = Vec::with_capacity(levels as usize);
    for level in 0..levels {
        let d = t
            .image_level_description(bytes, 0, level)
            .ok_or("missing mip level")?;
        let data = t
            .transcode_image_level(
                bytes,
                format.transcoder(),
                TranscodeParameters {
                    image_index: 0,
                    level_index: level,
                    ..Default::default()
                },
            )
            .map_err(|e| format!("transcoding failed: {e:?}"))?;
        out.push(Level {
            width: d.original_width,
            height: d.original_height,
            data,
        });
    }
    t.end_transcoding();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [(x * 255 / w) as u8, (y * 255 / h) as u8, 128, 255]
            })
            .collect()
    }

    #[test]
    fn encodes_and_transcodes_to_every_gpu_format() {
        let (w, h) = (64, 32);
        let src = gradient(w, h);
        for kind in [ImageKind::Colour, ImageKind::Normal] {
            let file = encode(&src, w, h, kind).unwrap();
            assert!(is_basis(&file));
            assert!(file.len() < src.len() / 2, "{kind:?}: {} bytes", file.len());
            let (iw, ih, levels) = info(&file).unwrap();
            assert_eq!((iw, ih), (w, h));
            assert!(levels >= 6, "mipmaps: {levels}");
            for f in [
                GpuFormat::Astc4x4,
                GpuFormat::Etc2Rgba,
                GpuFormat::Bc7,
                GpuFormat::Rgba8,
            ] {
                let lv = transcode(&file, f).unwrap();
                assert_eq!(lv.len() as u32, levels);
                let blocks = w.div_ceil(f.block()) * h.div_ceil(f.block());
                assert_eq!(lv[0].data.len() as u32, blocks * f.block_bytes(), "{f:?}");
            }
            // Decoded back, the picture survives.
            let rgba = &transcode(&file, GpuFormat::Rgba8).unwrap()[0].data;
            let err: f64 = rgba
                .iter()
                .zip(&src)
                .map(|(a, b)| (*a as f64 - *b as f64).abs())
                .sum::<f64>()
                / src.len() as f64;
            assert!(err < 12.0, "{kind:?}: mean error {err:.1}");
        }
    }
}
